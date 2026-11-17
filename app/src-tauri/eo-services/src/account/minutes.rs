//! A measured session minute by minute: each step between two consecutive
//! readings, what the readings saw in it against what the chat log said.
//!
//! Loot TT stays the chat log's; nothing here changes it. What the series
//! catches is stock and slips, in the step they happen:
//!
//! - **Loot.** The chat log's loot lines (kill and harvest loot still
//!   counted, an enhancer's returned Shrapnel aside) are matched item by
//!   item against what arrived, by name and quantity (and, for single
//!   items, TT), oldest line first. A line may be matched in its own step or
//!   the next one, since the site shows play up to about a minute late.
//!   What arrived and nothing explains is **arrived, not looted** (a
//!   purchase, a trade, a gift, a pickup). These explain an arrival without
//!   being this session's loot: an enhancer's returned Shrapnel (timed at
//!   the kill after the break, so it may have arrived a step before),
//!   another session's loot looted within the site's lag before this one
//!   started, a quest reward recorded in the session from its completion,
//!   Shrapnel turned into ammo in the same step as Shrapnel looted (read
//!   back from the ammo row it paid), and what a strongbox opening produced
//!   or may have produced, in its own step or one recorded or proposed
//!   nearby. Universal Ammo is never flagged: what arrives of it is a
//!   conversion's, an opening's, or a top-up the measure already reads.
//! - **Loot that never arrived** is flagged only where the readings prove
//!   it: none of the line arrived by the end of the next step, and no
//!   holding of its item fell in any step from its looting through then
//!   (used, sold, traded, converted). Otherwise it is unconfirmed, never
//!   flagged.
//! - **Weapon and healing.** What the step measured (as the session's costs
//!   would bill it) beside the tracked spend of the kills ended and the
//!   heals observed in it. A fight that runs across readings puts its
//!   measured spend in several steps and its tracked spend in the one it
//!   ended in, so steps are never compared one by one: see [`stretches`].
//! - **Armour** has no tracked counterpart: measured only.
//!
//! Exact or refused: a step across a gap between readings is one step with
//! its span and no measured lanes; a session whose readings do not cover it
//! whole gives the steps it has with its coverage, never invented minutes.
//!
//! Computed on a reader. The arrivals flagged become proposals, and the
//! pending ones whose flag is gone are withdrawn, in a short write of their
//! own (see [`super::proposals::sync_arrivals`]), so each flag carries a
//! handle the player decides it by.

use std::collections::{BTreeMap, BTreeSet};

use super::amount::{checked, Amount, Precision, MICRO_PER_PEC, MICRO_PER_PED};
use super::costs::{Basis, CostBucket, SessionCosts, Unmeasured};
use super::diff::Span;
use super::follow::history_ends_at;
use super::measure::{
    ammo_for_shrapnel, measure_steps, Edge, GainMark, NotableEvent, SessionMeasure, SessionWindow,
    GAP_ABOVE_SECONDS,
};
use super::proposals::{
    local_day, session_arrivals, ArrivalDecision, ArrivalProposal, ArrivalSeen, NotProposed,
    OpeningProposal, ProposalId, ProposalKind, PAYLOAD_VERSION,
};
use super::reading::HoldingKey;
use super::roles::{KnownRoles, Stacking, SHRAPNEL, STRONGBOX_KEY, UNIVERSAL_AMMO};
use super::sessions::tracked_session;
use super::store::{used_readings, ReadingId, StoredReading};
use super::AccountError;

/// The gap between measured and tracked spend that a stretch must move
/// beyond, besides a PEC for every item whose value the site truncated in
/// it and the largest single tracked event in it: what the site's lag and
/// the clocks the events are timed by move a running total by.
pub const GAP_NOISE_PEC: i64 = 5;

/// How many points in a row the gap must stay moved away for its stretch
/// to be flagged.
pub const PERSIST_POINTS: usize = 3;

/// The most steps a block of the gap runs over before it is closed with no
/// tracked event: longer than a fight.
pub const BLOCK_STEPS: usize = 5;

/// How long the site may take to show play: loot looted this long before a
/// session began may arrive in its first steps.
pub const SITE_LAG_SECONDS: f64 = 60.0;

/// How far from a step a strongbox opening proposed or recorded explains
/// what arrived in it.
pub const OPENING_NEARBY_SECONDS: f64 = GAP_ABOVE_SECONDS;

/// Where a loot line was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LootSource {
    Kill,
    Harvest,
}

/// What a recorded line is to the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineRole {
    /// The session's own loot.
    Loot,
    /// An enhancer's returned Shrapnel: it arrives, but is not loot.
    EnhancerReturn,
    /// Another session's loot, looted within the site's lag before this one
    /// began: it may arrive in this one's first steps.
    EarlierSession,
}

/// One loot line, as recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct LootLine {
    pub source: LootSource,
    /// The line's own row.
    pub id: i64,
    pub item: String,
    pub quantity: i64,
    /// Its TT, as the chat log gave it.
    pub value: f64,
    /// When the kill or swing it came from ended (unix seconds).
    pub at: f64,
    pub role: LineRole,
}

/// A quest reward's item recorded in the session.
#[derive(Debug, Clone, PartialEq)]
pub struct RewardItem {
    pub item: String,
    pub quantity: i64,
    /// When the quest was completed (unix seconds).
    pub at: f64,
}

/// A strongbox opening proposed or recorded: when, and the names of what
/// it produced or may have produced.
#[derive(Debug, Clone, PartialEq)]
pub enum OpeningNearby {
    /// Proposed from a step (or steps) of the readings.
    Seen {
        between: Span,
        items: BTreeSet<String>,
    },
    /// Recorded by the player, on a day.
    Recorded {
        day: String,
        recorded_at: f64,
        items: BTreeSet<String>,
    },
}

/// What the app recorded of the session and around it, as the series reads
/// it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionRecord {
    pub loot: Vec<LootLine>,
    /// Each kill's weapon spend, timed when it ended.
    pub kills: Vec<(f64, f64)>,
    /// Each paid heal's cost with its standing corrections, timed when it
    /// was observed.
    pub heals: Vec<(f64, f64)>,
    pub rewards: Vec<RewardItem>,
    pub openings: Vec<OpeningNearby>,
    /// The latest reading stored before steps were recognised: a step up
    /// to it is history, never proposed.
    pub history_ends_at: i64,
}

/// Why a step's bucket has no measured figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepUnmeasured {
    /// The step spans a gap between readings: what changed across it is not
    /// this step's alone.
    Gap,
    /// Universal Ammo was topped up in the step: what was fired in it is
    /// not known from the rows.
    AmmoToppedUp,
    /// The costs' own reason the bucket is not measured.
    Bucket(Unmeasured),
}

/// A step's measured figure for a bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepFigure {
    Measured(Amount),
    Unmeasured(StepUnmeasured),
}

impl StepFigure {
    pub fn amount(self) -> Option<Amount> {
        match self {
            StepFigure::Measured(amount) => Some(amount),
            StepFigure::Unmeasured(_) => None,
        }
    }
}

/// A bucket with a tracked counterpart, in one step.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    pub measured: StepFigure,
    /// What the tracked events timed in the step cost, in PED.
    pub tracked: f64,
    /// The items whose truncated values the measured figure rests on.
    truncated: BTreeSet<String>,
    /// How many tracked events are timed in the step, and the largest.
    events: usize,
    largest_event: f64,
}

/// Something that arrived in a step that nothing recorded explains.
#[derive(Debug, Clone, PartialEq)]
pub struct UnexplainedArrival {
    pub key: HoldingKey,
    pub units: i64,
    pub amount: Amount,
    /// The PED Card fell by its TT in the step: bought at the Trade
    /// Terminal.
    pub bought_at_tt: bool,
    /// What the player decided of it, or why it is not offered.
    pub decision: ArrivalDecision,
}

/// Why the existing loot correction is not offered for loot that never
/// arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoCorrection {
    /// Harvest loot has no correction.
    HarvestLoot,
    /// The correction removes the item from the whole session, and other
    /// lines of it arrived, are not proven missing, or are an enhancer's
    /// return.
    OtherLinesArrived,
}

/// What correcting loot that never arrived takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LootCorrection {
    /// The existing loot correction, which takes the item out of the
    /// session's loot, removes exactly the lines that never arrived.
    Offered {
        session_id: String,
        item_name: String,
    },
    NotOffered(NoCorrection),
}

/// Loot lines the chat log recorded that the readings prove never arrived.
#[derive(Debug, Clone, PartialEq)]
pub struct MissingLoot {
    pub item: String,
    pub units: i64,
    /// The lines' TT, from the chat log.
    pub value: f64,
    /// The step it was looted in.
    pub looted_in: Span,
    /// The lines, by where they were recorded and their own row.
    pub lines: Vec<(LootSource, i64)>,
    pub correction: LootCorrection,
}

/// One step of the series.
#[derive(Debug, Clone, PartialEq)]
pub struct Minute {
    /// The later reading of the step, and the one before it.
    pub reading: ReadingId,
    pub prev_reading: ReadingId,
    pub between: Span,
    /// The readings are further apart than a gap allows: one step across
    /// it.
    pub gap: bool,
    /// False for the last step of a running session: what arrives in it may
    /// still be explained.
    pub settled: bool,
    /// The TT of the session's loot lines timed in the step, in PED.
    pub looted: f64,
    /// The TT of what arrived in the step, Universal Ammo aside.
    pub arrived: Amount,
    pub arrived_not_looted: Vec<UnexplainedArrival>,
    pub looted_never_arrived: Vec<MissingLoot>,
    pub weapon: Lane,
    pub healing: Lane,
    pub armour: StepFigure,
}

/// A stretch of steps where the measured spend's gap to the tracked one
/// moved away from its usual slope.
#[derive(Debug, Clone, PartialEq)]
pub struct Stretch {
    /// The steps it covers, by their position in the series.
    pub first_step: usize,
    pub last_step: usize,
    pub between: Span,
    /// What the readings measured and the tracked events cost over it.
    pub measured: Amount,
    pub tracked: f64,
    /// How far the gap moved over it, beyond the session's usual slope, in
    /// micro-PED (measured above tracked is positive).
    pub drift_micro_ped: i64,
}

/// How well the readings cover the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinutesCoverage {
    pub opening: Edge,
    pub closing: Edge,
    pub reading_count: usize,
    pub gap_count: usize,
}

impl MinutesCoverage {
    /// Both ends read, at least two readings, and no gap.
    pub fn is_complete(&self) -> bool {
        self.opening == Edge::Read
            && self.closing == Edge::Read
            && self.gap_count == 0
            && self.reading_count >= 2
    }
}

/// A measured session, minute by minute.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionMinutes {
    pub session_id: String,
    pub running: bool,
    pub coverage: MinutesCoverage,
    pub steps: Vec<Minute>,
    pub weapon_stretches: Vec<Stretch>,
    pub healing_stretches: Vec<Stretch>,
    /// The TT of the session's loot no step can confirm or prove missing:
    /// timed after the last reading, in the last step with none after it,
    /// partly arrived, or of an item whose holdings fell meanwhile.
    pub loot_unconfirmed: f64,
    /// Pending, unclaimed arrival proposals of the session whose flag is
    /// gone: withdrawn as the read finishes.
    pub withdraw: Vec<ProposalId>,
}

impl SessionMinutes {
    /// The arrivals flagged that are to be stored as proposals.
    pub fn to_propose(&self) -> Vec<ArrivalSeen> {
        self.steps
            .iter()
            .flat_map(|step| {
                step.arrived_not_looted
                    .iter()
                    .filter(|arrival| {
                        arrival.decision == ArrivalDecision::NotProposed(NotProposed::Awaiting)
                    })
                    .map(move |arrival| ArrivalSeen {
                        reading: step.reading,
                        prev_reading: step.prev_reading,
                        between: step.between,
                        arrival: ArrivalProposal {
                            key: arrival.key.clone(),
                            units: arrival.units,
                            amount: arrival.amount,
                            bought_at_tt: arrival.bought_at_tt,
                        },
                    })
            })
            .collect()
    }

    /// Each arrival offered with what the player decided of it, as stored;
    /// the stored proposals no flag offers any more are to withdraw.
    pub fn with_stored(mut self, stored: &[super::proposals::StoredArrival]) -> SessionMinutes {
        let mut offered: BTreeSet<(i64, String)> = BTreeSet::new();
        for step in &mut self.steps {
            for arrival in &mut step.arrived_not_looted {
                if arrival.decision != ArrivalDecision::NotProposed(NotProposed::Awaiting) {
                    continue;
                }
                offered.insert((step.reading.0, arrival.key.name.clone()));
                if let Some(found) = stored.iter().find(|found| {
                    found.reading == step.reading && found.subject == arrival.key.name
                }) {
                    arrival.decision = found.decision.clone();
                }
            }
        }
        self.withdraw = stored
            .iter()
            .filter(|found| {
                found.withdrawable && !offered.contains(&(found.reading.0, found.subject.clone()))
            })
            .map(|found| found.id)
            .collect();
        self
    }
}

/// What the app recorded of the session `session_id` and around it:
/// its loot lines, another session's lines within the site's lag before it
/// began, its kills, heals and quest rewards, and strongbox openings
/// proposed or recorded around `bounds`. A read.
pub fn read_session_record(
    conn: &rusqlite::Connection,
    session_id: &str,
    window: &SessionWindow,
    bounds: Option<Span>,
) -> Result<SessionRecord, AccountError> {
    let mut record = SessionRecord {
        history_ends_at: history_ends_at(conn)?,
        ..SessionRecord::default()
    };
    let kill_line = |source_role: Option<LineRole>| {
        move |row: &rusqlite::Row| -> rusqlite::Result<LootLine> {
            let enhancer = row.get::<_, i64>(4)? != 0;
            Ok(LootLine {
                source: LootSource::Kill,
                id: row.get(0)?,
                item: row.get(1)?,
                quantity: row.get::<_, Option<i64>>(2)?.unwrap_or(1),
                value: row.get(3)?,
                at: row.get(5)?,
                role: source_role.unwrap_or(if enhancer {
                    LineRole::EnhancerReturn
                } else {
                    LineRole::Loot
                }),
            })
        }
    };
    {
        let mut stmt = conn.prepare(
            "SELECT l.id, l.item_name, l.quantity, l.value_ped, l.is_enhancer_shrapnel, \
                    k.timestamp \
             FROM kill_loot_items l JOIN counted_kills k ON k.id = l.kill_id \
             WHERE k.session_id = ?1 AND l.deactivated_at IS NULL",
        )?;
        let rows = stmt
            .query_map([session_id], kill_line(None))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        record.loot.extend(rows);
    }
    {
        let mut stmt = conn.prepare(
            "SELECT l.id, l.item_name, l.quantity, l.value_ped, 0, k.timestamp \
             FROM kill_loot_items l JOIN counted_kills k ON k.id = l.kill_id \
             WHERE k.session_id <> ?1 AND l.deactivated_at IS NULL \
               AND k.timestamp >= ?2 AND k.timestamp <= ?3",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    session_id,
                    window.started_at - SITE_LAG_SECONDS,
                    window.started_at
                ],
                kill_line(Some(LineRole::EarlierSession)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        record.loot.extend(rows);
    }
    {
        let mut stmt = conn.prepare(
            "SELECT l.id, l.item_name, l.quantity, l.value_ped, e.timestamp, e.session_id = ?1 \
             FROM harvest_loot_items l JOIN harvest_events e ON e.id = l.harvest_id \
             WHERE l.deactivated_at IS NULL \
               AND (e.session_id = ?1 \
                    OR (e.timestamp >= ?2 AND e.timestamp <= ?3))",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    session_id,
                    window.started_at - SITE_LAG_SECONDS,
                    window.started_at
                ],
                |row| {
                    Ok(LootLine {
                        source: LootSource::Harvest,
                        id: row.get(0)?,
                        item: row.get(1)?,
                        quantity: row.get::<_, Option<i64>>(2)?.unwrap_or(1),
                        value: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                        at: row.get(4)?,
                        role: if row.get::<_, bool>(5)? {
                            LineRole::Loot
                        } else {
                            LineRole::EarlierSession
                        },
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        record.loot.extend(rows);
    }
    {
        let mut stmt = conn.prepare(
            // every-kill: spend (each kill's weapon spend, timed by when it ended).
            "SELECT k.timestamp, \
                    COALESCE(SUM(COALESCE(ts.cost_per_shot, 0) * COALESCE(ts.shots_fired, 0)), 0) \
             FROM kills k LEFT JOIN kill_tool_stats ts ON ts.kill_id = k.id \
             WHERE k.session_id = ?1 GROUP BY k.id",
        )?;
        record.kills = stmt
            .query_map([session_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    {
        let mut stmt = conn.prepare(
            "SELECT a.observed_at, a.cost_ped \
                    + COALESCE((SELECT SUM(c.cost_delta_ped) FROM healing_corrections c \
                                WHERE c.activation_id = a.id AND c.session_id = ?1 \
                                  AND c.undone_at IS NULL), 0) \
             FROM healing_activations a WHERE a.session_id = ?1",
        )?;
        record.heals = stmt
            .query_map([session_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    {
        let mut stmt = conn.prepare(
            "SELECT ri.item_name, ri.quantity, c.completed_at \
             FROM session_quest_completion_reward_items ri \
             JOIN session_quest_completions c ON c.id = ri.completion_id \
             WHERE c.session_id = ?1 \
               AND NOT EXISTS (SELECT 1 FROM quest_reward_reversals rr \
                               WHERE rr.completion_id = c.id)",
        )?;
        record.rewards = stmt
            .query_map([session_id], |row| {
                Ok(RewardItem {
                    item: row.get(0)?,
                    quantity: row.get(1)?,
                    at: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    if let Some(bounds) = bounds {
        record.openings = openings_near(conn, &bounds)?;
    }
    Ok(record)
}

/// The strongbox openings proposed (and not dismissed) or recorded (and
/// not taken back) around `bounds`, with what each produced or may have.
fn openings_near(
    conn: &rusqlite::Connection,
    bounds: &Span,
) -> Result<Vec<OpeningNearby>, AccountError> {
    let mut found = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT seen_from, seen_to, payload_version, payload_json FROM account_proposals \
             WHERE kind = ?1 AND status <> 'dismissed' AND seen_to >= ?2 AND seen_from <= ?3",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    ProposalKind::Opening.as_str(),
                    bounds.from - OPENING_NEARBY_SECONDS,
                    bounds.to + OPENING_NEARBY_SECONDS
                ],
                |row| {
                    Ok((
                        row.get::<_, f64>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (from, to, version, payload) in rows {
            if version != PAYLOAD_VERSION {
                continue;
            }
            let Ok(opening) = serde_json::from_str::<OpeningProposal>(&payload) else {
                continue;
            };
            found.push(OpeningNearby::Seen {
                between: Span { from, to },
                items: opening
                    .contents
                    .iter()
                    .chain(&opening.possible_contents)
                    .map(|content| content.key.name.clone())
                    .collect(),
            });
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT o.opened_at, o.created_at, l.item_name \
             FROM strongbox_openings o \
             JOIN stock_conversion_lines l ON l.conversion_id = o.opening_conversion_id \
             WHERE o.undone_at IS NULL AND l.side = 'output' AND o.created_at >= ?1",
        )?;
        let rows = stmt
            .query_map([bounds.from], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut recorded: BTreeMap<(String, i64), BTreeSet<String>> = BTreeMap::new();
        for (day, recorded_at, item) in rows {
            recorded
                .entry((day, recorded_at.to_bits() as i64))
                .or_default()
                .insert(item);
        }
        for ((day, bits), items) in recorded {
            found.push(OpeningNearby::Recorded {
                day,
                recorded_at: f64::from_bits(bits as u64),
                items,
            });
        }
    }
    Ok(found)
}

/// A session's minutes, read from its stored readings and what the app
/// recorded of it, each flagged arrival with what was decided of it so far.
/// `None` when no tracked session has that id. A read: run it on a reader.
pub fn read_minutes(
    conn: &rusqlite::Connection,
    session_id: &str,
    roles: &KnownRoles,
) -> Result<Option<SessionMinutes>, AccountError> {
    let Some(session) = tracked_session(conn, session_id)? else {
        return Ok(None);
    };
    let used = used_readings(conn, session_id, &session.window)?;
    let bounds = match (used.readings.first(), used.readings.last()) {
        (Some(first), Some(last)) => Some(Span {
            from: first.reading.taken_at(),
            to: last.reading.taken_at(),
        }),
        _ => None,
    };
    let record = read_session_record(conn, session_id, &session.window, bounds)?;
    let running = session.active || session.window.ended_at.is_none();
    let minutes = minutes_of(
        session_id,
        &used.readings,
        (used.opening, used.closing),
        running,
        roles,
        &session.window,
        &record,
    )?;
    Ok(Some(
        minutes.with_stored(&session_arrivals(conn, session_id)?),
    ))
}

/// PED as micro-PED, to the nearest.
fn micro(ped: f64) -> i64 {
    (ped * MICRO_PER_PED as f64).round() as i64
}

/// The position of the step whose span holds `at` (after its first
/// reading, by its second), if any.
fn step_of(spans: &[Span], at: f64) -> Option<usize> {
    spans
        .iter()
        .position(|span| span.from < at && at <= span.to)
}

/// `of` units' share of `amount` for `units` of them: exact when it divides
/// exactly and the amount is, otherwise resting on the truncated values.
fn share(amount: Amount, units: i64, of: i64) -> Result<Amount, AccountError> {
    if units == of || of == 0 {
        return Ok(amount);
    }
    let scaled = i128::from(amount.micro_ped()) * i128::from(units);
    let whole = scaled % i128::from(of) == 0;
    let micro_ped = i64::try_from(scaled / i128::from(of)).map_err(|_| AccountError::Overflow)?;
    Ok(if whole && amount.precision() == Precision::Exact {
        Amount::exact(micro_ped)
    } else {
        Amount::WholePec { micro_ped }
    })
}

/// One thing that arrived in a step, as what was recorded is matched
/// against it.
struct Arriving {
    key: HoldingKey,
    units: i64,
    amount: Amount,
    /// One item per row: a line must match its TT too.
    single: bool,
    /// Possibly a strongbox opening's: matched, never flagged.
    possibly_strongbox: bool,
    bought_at_tt: bool,
    /// Units not yet explained.
    left: i64,
}

impl Arriving {
    /// Whether `line` may explain units of this arrival: the same item,
    /// and for single items the same TT within the PEC the site truncates.
    fn takes(&self, line: &LootLine) -> bool {
        if line.item != self.key.name {
            return false;
        }
        if !self.single || self.units == 0 {
            return true;
        }
        let per_unit = self.amount.micro_ped() / self.units;
        let line_unit = micro(line.value / line.quantity.max(1) as f64);
        (per_unit - line_unit).abs() <= MICRO_PER_PEC
    }
}

/// A recorded line waiting to be matched.
struct Waiting {
    line: usize,
    step: usize,
    left: i64,
}

/// What arrived in a step, Universal Ammo aside.
fn arrivals_of(measure: &SessionMeasure) -> Vec<Arriving> {
    let mut arriving: Vec<Arriving> = measure
        .gained
        .iter()
        .filter(|gain| gain.key.name != UNIVERSAL_AMMO && gain.units > 0)
        .map(|gain| Arriving {
            key: gain.key.clone(),
            units: gain.units,
            amount: gain.amount,
            single: gain.stacking == Stacking::Single,
            possibly_strongbox: gain.mark == GainMark::PossiblyStrongbox,
            bought_at_tt: false,
            left: gain.units,
        })
        .collect();
    for event in &measure.events {
        if let NotableEvent::Purchase { items, .. } = event {
            arriving.extend(
                items
                    .iter()
                    .filter(|item| item.key.name != UNIVERSAL_AMMO && item.units > 0)
                    .map(|item| Arriving {
                        key: item.key.clone(),
                        units: item.units,
                        amount: item.amount,
                        single: false,
                        possibly_strongbox: false,
                        bought_at_tt: true,
                        left: item.units,
                    }),
            );
        }
    }
    // What certainly arrived is matched first, so loot is never spent on
    // what an opening may have produced.
    arriving.sort_by_key(|arrival| arrival.possibly_strongbox);
    arriving
}

/// The names whose holdings fell in a step: used, sold, traded, left,
/// converted, listed as sold, or opened.
fn fell_in(measure: &SessionMeasure) -> BTreeSet<String> {
    let mut fell: BTreeSet<String> = measure
        .other_consumption
        .iter()
        .chain(&measure.unexplained_departures)
        .map(|item| item.key.name.clone())
        .collect();
    for event in &measure.events {
        match event {
            NotableEvent::Left { key, .. } | NotableEvent::ListingSold { key, .. } => {
                fell.insert(key.name.clone());
            }
            NotableEvent::Sale { items, .. } => {
                fell.extend(items.iter().map(|item| item.key.name.clone()));
            }
            NotableEvent::ShrapnelConverted { .. } => {
                fell.insert(SHRAPNEL.to_string());
            }
            NotableEvent::StrongboxOpening { opening, .. } => {
                fell.insert(STRONGBOX_KEY.to_string());
                fell.extend(opening.boxes.iter().map(|boxes| boxes.name.clone()));
            }
            _ => {}
        }
    }
    fell
}

/// Units of Shrapnel that left the step with nothing to explain them (no
/// conversion the measure read alone), and units of Universal Ammo that
/// arrived in it beyond what the measure credits.
fn shrapnel_and_ammo(measure: &SessionMeasure) -> (i64, i64) {
    let left = measure
        .unexplained_departures
        .iter()
        .filter(|item| item.key.name == SHRAPNEL)
        .map(|item| item.units)
        .sum();
    let ammo = measure
        .gained
        .iter()
        .filter(|gain| gain.key.name == UNIVERSAL_AMMO && gain.mark == GainMark::Arrived)
        .map(|gain| gain.units)
        .sum();
    (left, ammo)
}

/// The step's measured figure for `bucket`, as a closed session's costs
/// would bill it, with the items whose truncated values it rests on.
fn figure(costs: &SessionCosts, bucket: CostBucket, gap: bool) -> (StepFigure, BTreeSet<String>) {
    let truncated = costs
        .lines
        .iter()
        .filter(|line| line.bucket == bucket)
        .filter(|line| {
            line.amount()
                .is_some_and(|amount| amount.precision() == Precision::WholePec)
        })
        .map(|line| line.item.clone())
        .collect();
    let figure = match costs.bucket(bucket).map(|cost| cost.basis) {
        _ if gap => StepFigure::Unmeasured(StepUnmeasured::Gap),
        Some(Basis::Measured(amount)) => StepFigure::Measured(amount),
        Some(Basis::Mixed(_)) => StepFigure::Unmeasured(StepUnmeasured::AmmoToppedUp),
        Some(Basis::Unmeasured(reason)) => StepFigure::Unmeasured(StepUnmeasured::Bucket(reason)),
        // Every bucket is in a step's costs; one missing is not measured.
        None => StepFigure::Unmeasured(StepUnmeasured::Bucket(Unmeasured::TrackedOnly)),
    };
    (figure, truncated)
}

/// Whether a strongbox opening proposed or recorded explains `name`
/// arriving in the step `span`.
fn opened_nearby(openings: &[OpeningNearby], span: &Span, name: &str) -> bool {
    openings.iter().any(|opening| match opening {
        OpeningNearby::Seen { between, items } => {
            items.contains(name)
                && between.to >= span.from - OPENING_NEARBY_SECONDS
                && between.from <= span.to + OPENING_NEARBY_SECONDS
        }
        OpeningNearby::Recorded {
            day,
            recorded_at,
            items,
        } => {
            items.contains(name)
                && *recorded_at >= span.from
                && [span.from, span.to].iter().any(|at| {
                    local_day(*at) == *day
                        || crate::time::epoch_to_instant(*at)
                            .date_naive()
                            .format("%Y-%m-%d")
                            .to_string()
                            == *day
                })
        }
    })
}

/// The tracked events timed in `span`: their total, how many, and the
/// largest.
fn timed(events: &[(f64, f64)], span: &Span) -> (f64, usize, f64) {
    events
        .iter()
        .filter(|(at, _)| span.from < *at && *at <= span.to)
        .fold((0.0, 0, 0.0), |(total, count, largest), (_, cost)| {
            (total + cost, count + 1, f64::max(largest, *cost))
        })
}

/// The minutes of a session over its used `readings` (with whether they
/// read its ends, `edges`) against what the app recorded of it. Every
/// arrival offered is [`NotProposed::Awaiting`] here. Pure.
pub fn minutes_of(
    session_id: &str,
    readings: &[StoredReading],
    edges: (Edge, Edge),
    running: bool,
    roles: &KnownRoles,
    window: &SessionWindow,
    record: &SessionRecord,
) -> Result<SessionMinutes, AccountError> {
    let plain: Vec<_> = readings
        .iter()
        .map(|stored| stored.reading.clone())
        .collect();
    let measures = measure_steps(&plain, roles)?;
    let spans: Vec<Span> = readings
        .windows(2)
        .map(|pair| Span {
            from: pair[0].reading.taken_at(),
            to: pair[1].reading.taken_at(),
        })
        .collect();
    let n = spans.len();
    let fell: Vec<BTreeSet<String>> = measures.iter().map(fell_in).collect();
    let loot = &record.loot;

    // Each line in the step its time falls in. Another session's lines
    // wait from the first step; the session's own before the first reading
    // are in it already, and those after the last have no step.
    let mut lines_by_step: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut loot_unconfirmed = 0.0;
    let mut order: Vec<usize> = (0..loot.len()).collect();
    order.sort_by(|a, b| {
        let (a, b) = (&loot[*a], &loot[*b]);
        a.at.total_cmp(&b.at).then(a.id.cmp(&b.id))
    });
    for index in order {
        let line = &loot[index];
        let step = match line.role {
            LineRole::EarlierSession => (n > 0).then_some(0),
            _ => step_of(&spans, line.at),
        };
        match step {
            Some(step) => lines_by_step[step].push(index),
            None if line.role == LineRole::Loot
                && spans.last().is_none_or(|last| line.at > last.to) =>
            {
                loot_unconfirmed += line.value;
            }
            None => {}
        }
    }
    // Each reward from the step it was completed in, or the first when
    // completed within the site's lag before it.
    let mut rewards: Vec<(usize, String, i64)> = record
        .rewards
        .iter()
        .filter_map(|reward| {
            let step = step_of(&spans, reward.at).or_else(|| {
                spans
                    .first()
                    .filter(|first| {
                        reward.at <= first.from && reward.at >= first.from - SITE_LAG_SECONDS
                    })
                    .map(|_| 0)
            })?;
            Some((step, reward.item.clone(), reward.quantity))
        })
        .collect();

    let mut waiting: Vec<Waiting> = Vec::new();
    // Lines that never arrived: the step they are flagged in, the line and
    // its units.
    let mut missing: Vec<(usize, usize, i64)> = Vec::new();
    let mut steps: Vec<Minute> = Vec::with_capacity(n);
    // What the step before left unexplained, flagged once this step's
    // enhancer returns have had their say.
    let mut before: Option<Vec<Arriving>> = None;
    for (index, measure) in measures.iter().enumerate() {
        let span = spans[index];
        waiting.extend(lines_by_step[index].iter().map(|&line| Waiting {
            line,
            step: index,
            left: loot[line].quantity,
        }));

        // An enhancer's return is timed at the kill after the break, so the
        // Shrapnel may have arrived in the step before.
        if let Some(previous) = before.as_mut() {
            for wait in waiting.iter_mut().filter(|wait| {
                wait.step == index && loot[wait.line].role == LineRole::EnhancerReturn
            }) {
                for arrival in previous
                    .iter_mut()
                    .filter(|a| a.left > 0 && a.takes(&loot[wait.line]))
                {
                    let matched = wait.left.min(arrival.left);
                    wait.left -= matched;
                    arrival.left -= matched;
                }
            }
        }
        if let Some(previous) = before.take() {
            flag_arrivals(&mut steps[index - 1], previous, record, window)?;
        }

        let mut arriving = arrivals_of(measure);

        // Shrapnel looted and turned into ammo in the same step: the ammo
        // row it paid, read against the loot waiting.
        let (shrapnel_left, ammo_arrived) = shrapnel_and_ammo(measure);
        if ammo_arrived > 0 {
            let gained: i64 = arriving
                .iter()
                .filter(|arrival| arrival.key.name == SHRAPNEL)
                .map(|arrival| arrival.units)
                .sum();
            let net = checked(gained.checked_sub(shrapnel_left))?;
            for earlier_only in [false, true] {
                let take: Vec<usize> = waiting
                    .iter()
                    .enumerate()
                    .filter(|(_, wait)| {
                        loot[wait.line].item == SHRAPNEL
                            && wait.left > 0
                            && (!earlier_only || wait.step < index)
                    })
                    .map(|(position, _)| position)
                    .collect();
                let looted: i64 = take.iter().map(|&position| waiting[position].left).sum();
                let converted = checked(looted.checked_sub(net))?;
                if converted > 0 && looted > 0 && ammo_for_shrapnel(converted)? == ammo_arrived {
                    for position in take {
                        waiting[position].left = 0;
                    }
                    for arrival in arriving.iter_mut().filter(|a| a.key.name == SHRAPNEL) {
                        arrival.left = 0;
                    }
                    break;
                }
            }
        }

        for arrival in arriving.iter_mut() {
            // Recorded lines, oldest first.
            for wait in waiting.iter_mut() {
                if arrival.left == 0 {
                    break;
                }
                if wait.left == 0 || !arrival.takes(&loot[wait.line]) {
                    continue;
                }
                let matched = wait.left.min(arrival.left);
                wait.left -= matched;
                arrival.left -= matched;
            }
            // A quest reward recorded in the session, from its completion.
            for (step, item, units) in rewards.iter_mut() {
                if arrival.left == 0 {
                    break;
                }
                if *item != arrival.key.name || *units == 0 || *step > index || *step + 1 < index {
                    continue;
                }
                let matched = (*units).min(arrival.left);
                *units -= matched;
                arrival.left -= matched;
            }
            // What a strongbox opening proposed or recorded nearby produced.
            if arrival.left > 0 && opened_nearby(&record.openings, &span, &arrival.key.name) {
                arrival.possibly_strongbox = true;
            }
        }

        // Lines of the step before still waiting have had their next step.
        // Missing only where provable: none of the line arrived, and no
        // holding of its item fell from its looting through now.
        for wait in waiting.iter_mut().filter(|wait| wait.step < index) {
            let line = &loot[wait.line];
            if line.role == LineRole::Loot && wait.left > 0 {
                let untouched = wait.left == line.quantity
                    && (wait.step..=index).all(|step| !fell[step].contains(&line.item));
                if untouched {
                    missing.push((index, wait.line, wait.left));
                } else {
                    loot_unconfirmed += line.value * wait.left as f64 / line.quantity.max(1) as f64;
                }
            }
            wait.left = 0;
        }
        waiting.retain(|wait| wait.left > 0);

        let gap = !(0.0..=GAP_ABOVE_SECONDS).contains(&(span.to - span.from));
        let costs = SessionCosts::so_far(measure, roles, window.started_at)?;
        let (weapon, weapon_truncated) = figure(&costs, CostBucket::Weapon, gap);
        let (healing, healing_truncated) = figure(&costs, CostBucket::Healing, gap);
        let (armour, _) = figure(&costs, CostBucket::Armour, gap);
        let (kills, kill_count, largest_kill) = timed(&record.kills, &span);
        let (heals, heal_count, largest_heal) = timed(&record.heals, &span);
        let looted: f64 = lines_by_step[index]
            .iter()
            .map(|&line| &loot[line])
            .filter(|line| line.role == LineRole::Loot)
            .map(|line| line.value)
            .sum();
        let arrived = Amount::total(arriving.iter().map(|arrival| arrival.amount))?;
        steps.push(Minute {
            reading: readings[index + 1].header.id,
            prev_reading: readings[index].header.id,
            between: span,
            gap,
            settled: !(running && index + 1 == n),
            looted,
            arrived,
            arrived_not_looted: Vec::new(),
            looted_never_arrived: Vec::new(),
            weapon: Lane {
                measured: weapon,
                tracked: kills,
                truncated: weapon_truncated,
                events: kill_count,
                largest_event: largest_kill,
            },
            healing: Lane {
                measured: healing,
                tracked: heals,
                truncated: healing_truncated,
                events: heal_count,
                largest_event: largest_heal,
            },
            armour,
        });
        before = Some(arriving);
    }
    if let Some(previous) = before.take() {
        flag_arrivals(&mut steps[n - 1], previous, record, window)?;
    }
    // Lines of the last step have no step after them to arrive in.
    loot_unconfirmed += waiting
        .iter()
        .filter(|wait| loot[wait.line].role == LineRole::Loot)
        .map(|wait| {
            let line = &loot[wait.line];
            line.value * wait.left as f64 / line.quantity.max(1) as f64
        })
        .sum::<f64>();

    flag_missing(session_id, record, &spans, &missing, &mut steps);
    let weapon_stretches = lane_stretches(&steps, &spans, |step| &step.weapon)?;
    let healing_stretches = lane_stretches(&steps, &spans, |step| &step.healing)?;
    Ok(SessionMinutes {
        session_id: session_id.to_string(),
        running,
        coverage: MinutesCoverage {
            opening: edges.0,
            closing: edges.1,
            reading_count: readings.len(),
            gap_count: steps.iter().filter(|step| step.gap).count(),
        },
        steps,
        weapon_stretches,
        healing_stretches,
        loot_unconfirmed,
        withdraw: Vec::new(),
    })
}

/// Flag what a step left unexplained, one entry for each item, each with
/// why it is not offered where it is not.
fn flag_arrivals(
    step: &mut Minute,
    arriving: Vec<Arriving>,
    record: &SessionRecord,
    window: &SessionWindow,
) -> Result<(), AccountError> {
    let outside = step.between.to <= window.started_at
        || window
            .ended_at
            .is_some_and(|ended| step.between.from >= ended);
    let decision = ArrivalDecision::NotProposed(if outside {
        NotProposed::OutsideSession
    } else if step.reading.0 <= record.history_ends_at
        || step.prev_reading.0 <= record.history_ends_at
    {
        NotProposed::BeforeRecognition
    } else if !step.settled {
        NotProposed::Settling
    } else {
        NotProposed::Awaiting
    });
    for arrival in arriving
        .iter()
        .filter(|arrival| arrival.left > 0 && !arrival.possibly_strongbox)
    {
        let amount = share(arrival.amount, arrival.left, arrival.units)?;
        match step
            .arrived_not_looted
            .iter_mut()
            .find(|flag| flag.key.name == arrival.key.name)
        {
            Some(flag) => {
                flag.units = checked(flag.units.checked_add(arrival.left))?;
                flag.amount = flag.amount.plus(amount)?;
                flag.bought_at_tt &= arrival.bought_at_tt;
            }
            None => step.arrived_not_looted.push(UnexplainedArrival {
                key: arrival.key.clone(),
                units: arrival.left,
                amount,
                bought_at_tt: arrival.bought_at_tt,
                decision: decision.clone(),
            }),
        }
    }
    Ok(())
}

/// Turn the lines that never arrived into flags, one for each item in each
/// step, each with whether the existing loot correction removes exactly
/// them.
fn flag_missing(
    session_id: &str,
    record: &SessionRecord,
    spans: &[Span],
    missing: &[(usize, usize, i64)],
    steps: &mut [Minute],
) {
    // Lines proven missing, by item: what the correction, which takes an
    // item out of the session's kill loot, may remove.
    let mut proven: BTreeMap<&str, BTreeSet<i64>> = BTreeMap::new();
    for &(_, line, _) in missing {
        let line = &record.loot[line];
        if line.source == LootSource::Kill {
            proven
                .entry(line.item.as_str())
                .or_default()
                .insert(line.id);
        }
    }
    let correction = |lines: &[&LootLine]| -> LootCorrection {
        if lines.iter().any(|line| line.source == LootSource::Harvest) {
            return LootCorrection::NotOffered(NoCorrection::HarvestLoot);
        }
        let item = lines[0].item.as_str();
        let removable = proven.get(item);
        let every_line_missing = record
            .loot
            .iter()
            .filter(|line| {
                line.source == LootSource::Kill
                    && line.role != LineRole::EarlierSession
                    && line.item == item
            })
            .all(|line| removable.is_some_and(|ids| ids.contains(&line.id)));
        if every_line_missing {
            LootCorrection::Offered {
                session_id: session_id.to_string(),
                item_name: item.to_string(),
            }
        } else {
            LootCorrection::NotOffered(NoCorrection::OtherLinesArrived)
        }
    };
    let mut grouped: BTreeMap<(usize, String), Vec<usize>> = BTreeMap::new();
    for &(step, line, _) in missing {
        grouped
            .entry((step, record.loot[line].item.clone()))
            .or_default()
            .push(line);
    }
    for ((step, item), parts) in grouped {
        let lines: Vec<&LootLine> = parts.iter().map(|line| &record.loot[*line]).collect();
        let looted_in = step
            .checked_sub(1)
            .and_then(|before| spans.get(before))
            .copied()
            .unwrap_or(spans[step]);
        steps[step].looted_never_arrived.push(MissingLoot {
            correction: correction(&lines),
            item,
            units: lines.iter().map(|line| line.quantity).sum(),
            value: lines.iter().map(|line| line.value).sum(),
            looted_in,
            lines: lines.iter().map(|line| (line.source, line.id)).collect(),
        });
    }
}

/// One step's part in a lane's gap.
#[derive(Debug, Clone, PartialEq)]
pub struct StepGap {
    /// Measured less tracked, in micro-PED; `None` for a step the readings
    /// did not bill, or one across a gap, which breaks the series.
    pub residual: Option<i64>,
    /// The items whose truncated values the measured figure rests on.
    pub truncated: BTreeSet<String>,
    /// How many tracked events are timed in the step, and the largest one's
    /// cost in micro-PED.
    pub events: usize,
    pub largest_event: i64,
}

/// The lane's stretches over the steps.
fn lane_stretches(
    steps: &[Minute],
    spans: &[Span],
    lane: impl Fn(&Minute) -> &Lane,
) -> Result<Vec<Stretch>, AccountError> {
    let gaps: Vec<StepGap> = steps
        .iter()
        .map(|step| {
            let lane = lane(step);
            StepGap {
                residual: lane
                    .measured
                    .amount()
                    .map(|amount| amount.micro_ped() - micro(lane.tracked)),
                truncated: lane.truncated.clone(),
                events: lane.events,
                largest_event: micro(lane.largest_event),
            }
        })
        .collect();
    stretches(&gaps)
        .into_iter()
        .map(|(first, last, drift)| {
            let measured = Amount::total(
                steps[first..=last]
                    .iter()
                    .filter_map(|step| lane(step).measured.amount()),
            )?;
            Ok(Stretch {
                first_step: first,
                last_step: last,
                between: Span {
                    from: spans[first].from,
                    to: spans[last].to,
                },
                measured,
                tracked: steps[first..=last]
                    .iter()
                    .map(|step| lane(step).tracked)
                    .sum(),
                drift_micro_ped: drift,
            })
        })
        .collect()
}

/// A run of steps the gap is read over as one: closed by a tracked event
/// (the kill a fight's spend is tracked in has landed), by a step with
/// nothing measured or tracked, or after [`BLOCK_STEPS`].
struct Block {
    first: usize,
    last: usize,
    residual: i64,
}

/// The stretches of steps over which the gap between measured and tracked
/// spend moved away from its usual slope: `(first step, last step, how far
/// it moved in micro-PED)`.
///
/// A fight's measured spend lands in the steps it ran over and its tracked
/// spend in the step its kill landed in, so the gap is read only at the end
/// of blocks of steps that close on a tracked event ([`Block`]). A step the
/// readings did not bill, or one across a gap, breaks the series: the gap
/// is followed afresh after it. The usual slope is the median rate of the
/// blocks, so a steady bias all session is no stretch.
///
/// From a level, the cumulative gap must leave it by more than the floor
/// ([`GAP_NOISE_PEC`], a PEC for every item whose truncated value the steps
/// since rest on, since truncation telescopes over an item's run of
/// values, and the largest single tracked event since) and stay beyond it,
/// the same way, for [`PERSIST_POINTS`] block ends in a row. The stretch runs
/// from the last block end at the level to the first from which the gap
/// holds still, within the noise, for as many block ends; it is flagged
/// only when the gap moved by more than the floor over it, and the level
/// moves there.
pub fn stretches(steps: &[StepGap]) -> Vec<(usize, usize, i64)> {
    // The series, broken where a step was not billed, in blocks.
    let mut segments: Vec<Vec<Block>> = Vec::new();
    let mut current: Vec<Block> = Vec::new();
    let mut open: Option<Block> = None;
    for (index, step) in steps.iter().enumerate() {
        let Some(residual) = step.residual else {
            current.extend(open.take());
            if !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
            continue;
        };
        let block = open.get_or_insert(Block {
            first: index,
            last: index,
            residual: 0,
        });
        block.last = index;
        block.residual += residual;
        let idle = residual == 0 && step.events == 0;
        if step.events > 0 || idle || block.last + 1 - block.first >= BLOCK_STEPS {
            current.extend(open.take());
        }
    }
    current.extend(open.take());
    if !current.is_empty() {
        segments.push(current);
    }

    // The usual slope, per step: the median of the blocks' rates.
    let mut rates: Vec<i64> = segments
        .iter()
        .flatten()
        .map(|block| block.residual / (block.last + 1 - block.first) as i64)
        .collect();
    if rates.is_empty() {
        return Vec::new();
    }
    rates.sort_unstable();
    let middle = rates.len() / 2;
    let usual = if rates.len().is_multiple_of(2) {
        (rates[middle - 1] + rates[middle]) / 2
    } else {
        rates[middle]
    };

    let noise = GAP_NOISE_PEC * MICRO_PER_PEC;
    let mut found = Vec::new();
    for blocks in &segments {
        let points = blocks.len();
        // The cumulative gap at each block end, the usual slope taken out:
        // at point p, after the first p blocks.
        let mut at = vec![0i64; points + 1];
        for (index, block) in blocks.iter().enumerate() {
            let length = (block.last + 1 - block.first) as i64;
            at[index + 1] = at[index] + block.residual - usual * length;
        }
        let floor = |from: usize, to: usize| -> i64 {
            let covered = &steps[blocks[from].first..=blocks[to - 1].last];
            let items: BTreeSet<&String> = covered
                .iter()
                .flat_map(|step| step.truncated.iter())
                .collect();
            let largest = covered
                .iter()
                .map(|step| step.largest_event)
                .max()
                .unwrap_or(0);
            noise + items.len() as i64 * MICRO_PER_PEC + largest
        };
        let (mut level, mut anchor) = (0i64, 0usize);
        let mut p = 1;
        while p <= points {
            let away = at[p] - level;
            if away.abs() <= floor(anchor, p) {
                p += 1;
                continue;
            }
            let direction = away.signum();
            let persists = (p..p + PERSIST_POINTS).all(|q| {
                q <= points
                    && (at[q] - level).signum() == direction
                    && (at[q] - level).abs() > floor(anchor, q)
            });
            if !persists {
                p += 1;
                continue;
            }
            let start = (anchor..p)
                .rev()
                .find(|&s| (at[s] - level).abs() <= noise)
                .unwrap_or(anchor);
            let settle = (p..=points)
                .find(|&q| {
                    q + PERSIST_POINTS <= points
                        && (1..=PERSIST_POINTS).all(|k| (at[q + k] - at[q]).abs() <= noise)
                })
                .unwrap_or(points);
            let drift = at[settle] - level;
            if settle > start && drift.abs() > floor(start, settle) {
                found.push((blocks[start].first, blocks[settle - 1].last, drift));
            }
            level = at[settle];
            anchor = settle;
            p = settle + 1;
        }
    }
    found
}
