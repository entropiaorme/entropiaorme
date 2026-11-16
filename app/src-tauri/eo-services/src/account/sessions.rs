//! Tracked sessions as the account readings measure them: one session's
//! measure, the page of measured sessions beside their tracked figures,
//! and what happens to the readings when a session is deleted.
//!
//! A session's window is read from its own row (`tracking_sessions`) at
//! the moment of use, so the measure covers exactly the span the session
//! was tracked over. A session has readings when any stored reading is
//! tagged with it. Its measure is served from the cache when the current
//! rules sealed it. Otherwise it is computed from the stored readings on a
//! reader; a session that has ended and is no longer being read (the app
//! closed while it waited for the closing readings, or the connection
//! ended first) is sealed then, its measure handed back for the caller to
//! cache in a short write of its own. A session still being tracked is
//! computed on every read and never cached. Computing never happens while
//! the writer is held.
//!
//! The tracked figures beside a measure are read from the session's
//! summary as it stands: nothing here writes or refreshes a tracked
//! figure.

use std::collections::BTreeMap;

use rusqlite::OptionalExtension;

use super::amount::{Amount, Pec};
use super::composite::RepairCost;
use super::measure::{AmmoSplit, CoverageLevel, SessionMeasure, SessionWindow};
use super::roles::{KnownRoles, Role};
use super::store::{
    cache_session_measure_if_current, cached_measure, compute_session_measure, CachedMeasure,
    ComputedMeasure,
};
use super::AccountError;
use crate::game_data_store::GameDataStore;
use crate::session_scope::SessionExclusion;

/// The measured sessions listed on one page by default, and at most.
pub const MEASURED_PAGE_DEFAULT: i64 = 50;
pub const MEASURED_PAGE_MAX: i64 = 50;

/// A tracked session's own row, as the measure needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackedSession {
    pub window: SessionWindow,
    pub active: bool,
}

/// One tracked session's window and whether it still runs; `None` when no
/// such session exists.
pub fn tracked_session(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<TrackedSession>, AccountError> {
    Ok(conn
        .query_row(
            "SELECT started_at, ended_at, is_active FROM tracking_sessions WHERE id = ?1",
            [session_id],
            |row| {
                Ok(TrackedSession {
                    window: SessionWindow {
                        started_at: row.get(0)?,
                        ended_at: row.get(1)?,
                    },
                    active: row.get::<_, i64>(2)? != 0,
                })
            },
        )
        .optional()?)
}

/// Whether the session's measure is sealed.
pub fn is_sealed(conn: &rusqlite::Connection, session_id: &str) -> Result<bool, AccountError> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM account_session_measures \
                        WHERE session_id = ?1 AND sealed_at IS NOT NULL)",
        [session_id],
        |row| row.get::<_, bool>(0),
    )?)
}

/// Whether any stored reading is tagged with the session.
pub fn has_readings(conn: &rusqlite::Connection, session_id: &str) -> Result<bool, AccountError> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM account_readings WHERE session_id = ?1)",
        [session_id],
        |row| row.get::<_, bool>(0),
    )?)
}

/// The session tracked now, if one is: the one the app was tracking when
/// it last stopped, when nothing has closed it since.
pub fn active_session(conn: &rusqlite::Connection) -> Result<Option<String>, AccountError> {
    Ok(conn
        .query_row(
            // id-order: tiebreak (the latest start wins; id splits equal instants).
            "SELECT id FROM tracking_sessions WHERE is_active = 1 \
             ORDER BY started_at DESC, id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

/// Let go of a session about to be deleted: its cached measure goes, and
/// its readings stay, no longer tagged with it (a later reading may be
/// stored as the changes against one of them), as do the proposals seen
/// during it. Run inside the deletion's own transaction.
pub fn release_session(conn: &rusqlite::Connection, session_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM account_session_measures WHERE session_id = ?1",
        [session_id],
    )?;
    conn.execute(
        "UPDATE account_proposals SET session_id = NULL WHERE session_id = ?1",
        [session_id],
    )?;
    conn.execute(
        "UPDATE account_readings SET session_id = NULL WHERE session_id = ?1",
        [session_id],
    )?;
    Ok(())
}

/// One session's measure, as [`session_measure`] reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum MeasureRead {
    /// No tracked session has that id.
    UnknownSession,
    /// The session has no readings.
    NoReadings,
    Measured {
        measure: Box<SessionMeasure>,
        /// Final: the session has ended and its closing readings are in.
        sealed: bool,
    },
}

/// A measure sealed on a read, for the caller to cache.
#[derive(Debug, Clone, PartialEq)]
pub struct ToSeal {
    pub session_id: String,
    pub computed: ComputedMeasure,
    pub sealed_at: f64,
}

/// Read one session's measure on a reader. `being_read` names the session
/// the connection is reading now, which is never sealed here. A measure
/// sealed by this read comes back in the second place, to be cached with
/// [`cache_sealed`].
pub fn session_measure(
    conn: &rusqlite::Connection,
    session_id: &str,
    roles: &KnownRoles,
    now: f64,
    being_read: Option<&str>,
) -> Result<(MeasureRead, Option<ToSeal>), AccountError> {
    let Some(session) = tracked_session(conn, session_id)? else {
        return Ok((MeasureRead::UnknownSession, None));
    };
    if !has_readings(conn, session_id)? {
        return Ok((MeasureRead::NoReadings, None));
    }
    let (measure, sealed, to_seal) =
        measure_of(conn, session_id, &session, roles, now, being_read)?;
    Ok((
        MeasureRead::Measured {
            measure: Box::new(measure),
            sealed,
        },
        to_seal,
    ))
}

/// Cache the measures a read sealed, each only while it is still the
/// truth (see [`cache_session_measure_if_current`]): a short write. Returns
/// how many were written; one that no longer holds is left for the next
/// read to compute again.
pub fn cache_sealed(
    conn: &mut rusqlite::Connection,
    sealed: &[ToSeal],
    computed_at: f64,
) -> Result<usize, AccountError> {
    let mut written = 0;
    for seal in sealed {
        if cache_session_measure_if_current(
            conn,
            &seal.session_id,
            &seal.computed,
            Some(seal.sealed_at),
            computed_at,
        )? {
            written += 1;
        }
    }
    Ok(written)
}

fn measure_of(
    conn: &rusqlite::Connection,
    session_id: &str,
    session: &TrackedSession,
    roles: &KnownRoles,
    now: f64,
    being_read: Option<&str>,
) -> Result<(SessionMeasure, bool, Option<ToSeal>), AccountError> {
    let sealed_before = match cached_measure(conn, session_id)? {
        CachedMeasure::Current {
            measure,
            sealed_at: Some(_),
        } => return Ok((*measure, true, None)),
        CachedMeasure::Current { sealed_at, .. } | CachedMeasure::Stale { sealed_at } => sealed_at,
        CachedMeasure::Missing => None,
    };
    let finished =
        !session.active && session.window.ended_at.is_some() && being_read != Some(session_id);
    let computed = compute_session_measure(conn, session_id, &session.window, roles)?;
    if !finished {
        return Ok((computed.measure, false, None));
    }
    let measure = computed.measure.clone();
    Ok((
        measure,
        true,
        Some(ToSeal {
            session_id: session_id.to_string(),
            computed,
            sealed_at: sealed_before.unwrap_or(now),
        }),
    ))
}

/// Compute the sealed measure of a session that has ended, on a reader.
/// `None` for a session that is gone, still runs, or has no readings.
pub fn measure_to_seal(
    conn: &rusqlite::Connection,
    session_id: &str,
    game_data: Option<&GameDataStore>,
    now: f64,
) -> Result<Option<ToSeal>, AccountError> {
    let Some(session) = tracked_session(conn, session_id)? else {
        return Ok(None);
    };
    if session.active || session.window.ended_at.is_none() || !has_readings(conn, session_id)? {
        return Ok(None);
    }
    let roles = KnownRoles::read(conn, game_data)?;
    Ok(Some(ToSeal {
        session_id: session_id.to_string(),
        computed: compute_session_measure(conn, session_id, &session.window, &roles)?,
        sealed_at: now,
    }))
}

/// How a measure's ammo adds up across its types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmmoTotal {
    /// What every type burned.
    Settled(Amount),
    /// A type's split into burned and paid back by an opening is not
    /// known, so neither is the total burned.
    Unsettled,
}

/// What repairs cost across a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairTotal {
    /// The sum of the repairs whose cost the PED Card's fall shows alone.
    pub known: Amount,
    /// The sum of the repairs whose cost is what remains of the Card's
    /// change beside a sale or a purchase.
    pub derived: Amount,
    /// Repairs whose cost could not be read.
    pub unknown: usize,
}

/// A measure's figures by category: what the review of many sessions sets
/// beside the tracked figures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeasureTotals {
    pub ammo: AmmoTotal,
    /// Weapons and their amplifiers.
    pub weapon_decay: Amount,
    pub healing_decay: Amount,
    /// Armour and its plates.
    pub armour_decay: Amount,
    /// Everything else that wore: attachments, implants, tools, and items
    /// the app has no role for.
    pub other_decay: Amount,
    pub other_consumption: Amount,
    /// Stacks that left with nothing to explain them: never a cost.
    pub unexplained_departures: Amount,
    pub repairs: RepairTotal,
    pub gained: Amount,
    pub gained_possibly_strongbox: Amount,
}

impl MeasureTotals {
    pub fn of(measure: &SessionMeasure) -> Result<MeasureTotals, AccountError> {
        let mut ammo = Some(Amount::NONE);
        for used in &measure.ammo {
            ammo = match (&used.split, ammo) {
                (AmmoSplit::Settled { burned, .. }, Some(total)) => Some(total.plus(*burned)?),
                _ => None,
            };
        }
        let decay_of = |roles: &[Role]| {
            Amount::total(
                measure
                    .decay
                    .iter()
                    .filter(|item| roles.contains(&item.role))
                    .map(|item| item.decay),
            )
        };
        let other_roles = [Role::Ammo, Role::OtherEquipment, Role::Material];
        let mut known = Amount::NONE;
        let mut derived = Amount::NONE;
        let mut unknown = 0;
        for timed in &measure.repairs {
            match timed.repair.cost {
                RepairCost::Paid { pec } => known = known.plus(repair_cost(pec)?)?,
                RepairCost::Derived { amount } => derived = derived.plus(amount)?,
                RepairCost::Unknown => unknown += 1,
            }
        }
        Ok(MeasureTotals {
            ammo: ammo.map_or(AmmoTotal::Unsettled, AmmoTotal::Settled),
            weapon_decay: decay_of(&[Role::Weapon, Role::Amplifier])?,
            healing_decay: decay_of(&[Role::Healing])?,
            armour_decay: decay_of(&[Role::Armour, Role::Plating])?,
            other_decay: decay_of(&other_roles)?,
            other_consumption: Amount::total(
                measure.other_consumption.iter().map(|item| item.amount),
            )?,
            unexplained_departures: Amount::total(
                measure
                    .unexplained_departures
                    .iter()
                    .map(|item| item.amount),
            )?,
            repairs: RepairTotal {
                known,
                derived,
                unknown,
            },
            gained: measure.gained_total,
            gained_possibly_strongbox: measure.gained_possibly_strongbox,
        })
    }
}

/// A session's tracked figures, as its summary holds them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackedFigures {
    pub weapon_cost: f64,
    pub heal_cost: f64,
    pub armour_cost: f64,
    pub loot_tt: f64,
    pub cycled: f64,
}

/// One measured session on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredSession {
    pub id: String,
    pub name: Option<String>,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub coverage: CoverageLevel,
    pub reading_count: usize,
    pub gap_count: usize,
    pub sealed: bool,
    pub exclusion: SessionExclusion,
    pub totals: MeasureTotals,
    /// `None` while the session has no summary (it still runs, or nothing
    /// was tracked in it).
    pub tracked: Option<TrackedFigures>,
}

/// A page of measured sessions, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredPage {
    pub sessions: Vec<MeasuredSession>,
    /// The seek key of the last session served, when another page follows.
    pub next: Option<(f64, String)>,
    /// Every session with readings.
    pub total: i64,
}

/// One page of the sessions with readings, newest first by their start,
/// keyset-paged over `(started_at, id)` as the session list is, at most
/// [`MEASURED_PAGE_MAX`] to a page. A read: each measure is read as
/// [`session_measure`] reads it, and the measures this read sealed come
/// back beside the page for the caller to cache (none when every measure
/// on the page was already cached).
pub fn measured_sessions(
    conn: &rusqlite::Connection,
    seek: Option<&(f64, String)>,
    limit: Option<i64>,
    game_data: Option<&GameDataStore>,
    now: f64,
    being_read: Option<&str>,
) -> Result<(MeasuredPage, Vec<ToSeal>), AccountError> {
    let page = limit
        .unwrap_or(MEASURED_PAGE_DEFAULT)
        .clamp(1, MEASURED_PAGE_MAX);
    const MEASURED: &str =
        "EXISTS (SELECT 1 FROM account_readings r WHERE r.session_id = tracking_sessions.id)";
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM tracking_sessions WHERE {MEASURED}"),
        [],
        |row| row.get(0),
    )?;
    let mut sql = format!(
        "SELECT id, session_name, started_at, ended_at, is_active, {} \
         FROM tracking_sessions WHERE {MEASURED}",
        crate::session_scope::EXCLUSION_COLUMNS
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    // id-order: tiebreak (the seek is by started_at; id splits equal instants).
    if let Some((started_at, id)) = seek {
        sql.push_str(" AND (started_at < ? OR (started_at = ? AND id < ?))");
        params.push(Box::new(*started_at));
        params.push(Box::new(*started_at));
        params.push(Box::new(id.clone()));
    }
    // id-order: tiebreak (newest start first; id splits equal instants).
    sql.push_str(" ORDER BY started_at DESC, id DESC LIMIT ?");
    params.push(Box::new(page + 1));
    type Row = (
        String,
        Option<String>,
        f64,
        Option<f64>,
        bool,
        SessionExclusion,
    );
    let rows: Vec<Row> = {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get::<_, i64>(4)? != 0,
                    crate::session_scope::read_exclusion(row, 5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let more = rows.len() as i64 > page;
    let kept = &rows[..rows.len().min(page as usize)];
    let roles = KnownRoles::read(conn, game_data)?;
    let mut sessions = Vec::with_capacity(kept.len());
    let mut sealed_now = Vec::new();
    for (id, name, started_at, ended_at, active, exclusion) in kept {
        let session = TrackedSession {
            window: SessionWindow {
                started_at: *started_at,
                ended_at: *ended_at,
            },
            active: *active,
        };
        let (measure, sealed, to_seal) = measure_of(conn, id, &session, &roles, now, being_read)?;
        sealed_now.extend(to_seal);
        sessions.push(MeasuredSession {
            id: id.clone(),
            name: name.clone(),
            started_at: *started_at,
            ended_at: *ended_at,
            coverage: measure.coverage.level,
            reading_count: measure.coverage.reading_count,
            gap_count: measure.coverage.gap_count,
            sealed,
            exclusion: exclusion.clone(),
            totals: MeasureTotals::of(&measure)?,
            tracked: tracked_figures(conn, id)?,
        });
    }
    let next = more
        .then(|| kept.last())
        .flatten()
        .map(|(id, _, started_at, ..)| (*started_at, id.clone()));
    Ok((
        MeasuredPage {
            sessions,
            next,
            total,
        },
        sealed_now,
    ))
}

fn tracked_figures(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<TrackedFigures>, AccountError> {
    Ok(conn
        .query_row(
            "SELECT weapon_cost, heal_cost, armour_cost, loot_tt, cycled_ped \
             FROM session_summaries WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok(TrackedFigures {
                    weapon_cost: row.get(0)?,
                    heal_cost: row.get(1)?,
                    armour_cost: row.get(2)?,
                    loot_tt: row.get(3)?,
                    cycled: row.get(4)?,
                })
            },
        )
        .optional()?)
}

/// A repair's cost as an amount: the PED Card's fall, in whole PEC.
pub fn repair_cost(cost: Pec) -> Result<Amount, AccountError> {
    Amount::whole_pec(cost)
}

/// Why a session with readings is left out of the whole-history summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftOutReason {
    /// Its readings do not cover it whole.
    PartialCoverage,
    /// It is excluded from the economy statistics.
    Excluded,
    /// Its measure is not sealed by the current rules yet.
    NotSealed,
}

impl LeftOutReason {
    pub const ALL: [LeftOutReason; 3] = [
        LeftOutReason::PartialCoverage,
        LeftOutReason::Excluded,
        LeftOutReason::NotSealed,
    ];
}

/// One category summed across sessions: the tracked figure beside the
/// measured one, over the sessions where both are known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SummaryPair {
    pub sessions: usize,
    pub tracked: f64,
    pub measured: Amount,
}

impl Default for SummaryPair {
    fn default() -> SummaryPair {
        SummaryPair {
            sessions: 0,
            tracked: 0.0,
            measured: Amount::NONE,
        }
    }
}

impl SummaryPair {
    fn add(&mut self, tracked: f64, measured: Amount) -> Result<(), AccountError> {
        self.sessions += 1;
        self.tracked += tracked;
        self.measured = self.measured.plus(measured)?;
        Ok(())
    }
}

/// A category only the readings measure, summed across sessions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SummaryMeasured {
    pub sessions: usize,
    pub measured: Amount,
}

/// The whole history of measured sessions, by category, beside the
/// tracked figures.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredSummary {
    /// Tracked weapon cost beside measured ammo burned and weapon and
    /// amplifier wear.
    pub weapon: SummaryPair,
    /// Tracked healing cost beside measured healing wear.
    pub healing: SummaryPair,
    /// Tracked armour cost beside measured armour and plate wear.
    pub armour: SummaryPair,
    /// Other equipment wear and other consumption, which only the
    /// readings measure.
    pub other: SummaryMeasured,
    /// Tracked loot TT beside what the readings saw arrive.
    pub loot: SummaryPair,
    /// Sessions that went into the summary.
    pub included: usize,
    /// Sessions with readings left out, by reason (every reason listed).
    pub left_out: Vec<(LeftOutReason, usize)>,
}

/// The ids of the tracked sessions that have readings.
pub fn sessions_with_readings(conn: &rusqlite::Connection) -> Result<Vec<String>, AccountError> {
    let mut stmt = conn.prepare(
        "SELECT id FROM tracking_sessions s \
         WHERE EXISTS (SELECT 1 FROM account_readings r WHERE r.session_id = s.id)",
    )?;
    let ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// Sum every session with readings whose measure is sealed by the current
/// rules, covers it whole, and counts in the economy statistics (the same
/// exclusion rule the cross-session statistics apply). A read: it reads
/// cached measures only, and computes none; a session whose cached measure
/// is missing, stale or unsealed is left out as not sealed. A category's
/// pair takes a session only when both sides are known for it: an
/// unsettled ammo split leaves it out of the weapon pair, and armour with
/// hits not yet recorded (`armour_unrecorded`) out of the armour pair.
pub fn measured_summary(
    conn: &rusqlite::Connection,
    armour_unrecorded: &std::collections::HashSet<String>,
) -> Result<MeasuredSummary, AccountError> {
    use crate::session_scope::{counts_on, StatsScope};
    let rows: Vec<(String, bool)> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT s.id, {} FROM tracking_sessions s \
             WHERE EXISTS (SELECT 1 FROM account_readings r WHERE r.session_id = s.id)",
            counts_on(StatsScope::Economy, "s")
        ))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut weapon = SummaryPair::default();
    let mut healing = SummaryPair::default();
    let mut armour = SummaryPair::default();
    let mut other = SummaryMeasured {
        sessions: 0,
        measured: Amount::NONE,
    };
    let mut loot = SummaryPair::default();
    let mut included = 0;
    let mut left_out: BTreeMap<u8, usize> = BTreeMap::new();
    let mut leave = |reason: LeftOutReason| {
        *left_out.entry(reason as u8).or_default() += 1;
    };
    for (id, counts) in rows {
        if !counts {
            leave(LeftOutReason::Excluded);
            continue;
        }
        let CachedMeasure::Current {
            measure,
            sealed_at: Some(_),
        } = cached_measure(conn, &id)?
        else {
            leave(LeftOutReason::NotSealed);
            continue;
        };
        if measure.coverage.level != CoverageLevel::Complete {
            leave(LeftOutReason::PartialCoverage);
            continue;
        }
        included += 1;
        let totals = MeasureTotals::of(&measure)?;
        other.sessions += 1;
        other.measured = other
            .measured
            .plus(totals.other_decay)?
            .plus(totals.other_consumption)?;
        let Some(tracked) = tracked_figures(conn, &id)? else {
            continue;
        };
        if let AmmoTotal::Settled(ammo) = totals.ammo {
            weapon.add(tracked.weapon_cost, ammo.plus(totals.weapon_decay)?)?;
        }
        healing.add(tracked.heal_cost, totals.healing_decay)?;
        if !armour_unrecorded.contains(&id) {
            armour.add(tracked.armour_cost, totals.armour_decay)?;
        }
        loot.add(tracked.loot_tt, totals.gained)?;
    }
    Ok(MeasuredSummary {
        weapon,
        healing,
        armour,
        other,
        loot,
        included,
        left_out: LeftOutReason::ALL
            .iter()
            .map(|reason| {
                (
                    *reason,
                    left_out.get(&(*reason as u8)).copied().unwrap_or(0),
                )
            })
            .collect(),
    })
}
