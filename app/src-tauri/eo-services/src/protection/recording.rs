//! Recording a protection cost and spreading it over sessions.
//!
//! A new recording weighs each ticked session by its hits past its stream's
//! position ([`super::position`]), up to the hits recorded when it is
//! confirmed. A session entirely before the position can still be
//! re-included in an unlimited recording (a piece left unrepaired last
//! time); it is then weighed by all of its hits. A limited reading cannot
//! reach back past its baseline, because the TT it measured was lost after
//! the baseline was taken.
//!
//! A session whose share of the stream the account readings measured is
//! already billed, so no recording covers it; and what a recording measures
//! that those sessions already billed is taken off before it is spread.
//! A recording with nothing left to bill, or with nothing unmeasured to
//! cover, is refused with the reason.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::OptionalExtension;

use super::position::{
    billed_from_account, measured_since, measured_stream, stream_position, unmeasured_since,
    BaselineOrigin, Billed, ReadingCache, StreamPosition,
};
use super::read::{read_cost_window, read_observation};
use super::{
    account_error, CandidateSession, CostKind, ObservationOutcome, ObservationSource,
    ProtectionError, ProtectionService, ProtectionStream, RecordingCandidates, RepairOutcome,
    StreamBacklog,
};
use crate::account::KnownRoles;
use crate::db::DbError;
use crate::session_costs::armour_measured_sql;

/// The reason a recording kept its cost off every session.
pub const UNATTRIBUTED_REASON: &str = "Not attributed to any session";

/// The reason an older baseline reset left a window's cost unknown. Only
/// history carries it now: a reset starts a fresh baseline and books
/// nothing.
pub(super) const RESET_UNPRICED_REASON: &str =
    "Baseline reset left prior defensive evidence without a measurable cost";

/// How many sessions from before the previous unlimited repair a recording
/// may offer to re-include. Older play is not a realistic repair target.
const EARLIER_SESSIONS: i64 = 20;

/// The newest recorded hit: a recording covers every hit up to here.
fn current_cursor(conn: &rusqlite::Connection) -> Result<i64, rusqlite::Error> {
    // id-order: cursor (the highest defence-event id seen so far).
    conn.query_row(
        "SELECT COALESCE(MAX(id), 0) FROM protection_defence_events",
        [],
        |row| row.get(0),
    )
}

/// The stream's own live windows, as a SQL predicate over `w`.
fn stream_filter(stream: ProtectionStream) -> String {
    match stream {
        ProtectionStream::Unlimited => "w.kind = 'repair' AND w.superseded_at IS NULL".to_string(),
        ProtectionStream::Limited { set_id } => {
            format!("w.kind = 'limited_decay' AND w.set_id = {set_id} AND w.superseded_at IS NULL")
        }
    }
}

fn candidate_row(row: &rusqlite::Row<'_>) -> Result<CandidateSession, rusqlite::Error> {
    Ok(CandidateSession {
        session_id: row.get(0)?,
        session_name: row.get(1)?,
        definition_id: row.get(2)?,
        definition_name: row.get(3)?,
        started_at: row.get(4)?,
        ended_at: row.get(5)?,
        hit_count: row.get(6)?,
        covered: row.get::<_, i64>(7)? != 0,
        unrecorded: row.get::<_, i64>(8)? != 0,
    })
}

pub(super) fn read_candidates(
    conn: &rusqlite::Connection,
    stream: ProtectionStream,
    roles: &KnownRoles,
) -> Result<RecordingCandidates, ProtectionError> {
    let position = stream_position(conn, stream, &mut ReadingCache::default())?;
    let baseline_tt_ped = position.baseline.as_ref().map(|b| b.tt_value_ped);
    let baseline_source = position.baseline.as_ref().map(|b| b.source());
    if matches!(stream, ProtectionStream::Limited { .. }) && position.baseline.is_none() {
        // A first reading is a baseline: there is nothing to spread yet.
        return Ok(RecordingCandidates {
            stream,
            since: None,
            baseline_tt_ped,
            baseline_source,
            incomplete_account_reading_at: position.incomplete_reading_at,
            billed_from_account_ped: 0.0,
            account_tolerance_ped: 0.0,
            measured_sessions: 0,
            sessions: Vec::new(),
            earlier: Vec::new(),
        });
    }
    let measured_sessions = measured_since(conn, stream, &position)?;
    let billed = billed_from_account(conn, stream, &position, roles)?;
    let covered = format!(
        "EXISTS (SELECT 1 FROM protection_cost_allocations a \
                 JOIN protection_cost_windows w ON w.id = a.window_id \
                 WHERE a.session_id = s.id AND {}), \
         NOT EXISTS ({})",
        stream_filter(stream),
        super::read::live_allocation("s.id")
    );
    let unmeasured = format!(
        "NOT {}",
        armour_measured_sql(measured_stream(stream), "s.id")
    );

    // id-order: cursor (hits on the far side of the stream's position).
    let fresh_sql = format!(
        "SELECT s.id, s.session_name, s.definition_id, def.name, s.started_at, s.ended_at, \
                COUNT(d.id), {covered} \
         FROM protection_defence_events d \
         JOIN tracking_sessions s ON s.id = d.session_id \
         LEFT JOIN session_definitions def ON def.id = s.definition_id \
         WHERE d.id > ?1 AND {unmeasured} \
         GROUP BY s.id \
         ORDER BY s.started_at, s.id"
    );
    let mut stmt = conn.prepare(&fresh_sql)?;
    let sessions = stmt
        .query_map([position.cursor], candidate_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let earlier = if matches!(stream, ProtectionStream::Unlimited) && position.cursor > 0 {
        // id-order: cursor (sessions whose every hit precedes the position).
        let earlier_sql = format!(
            "SELECT s.id, s.session_name, s.definition_id, def.name, s.started_at, s.ended_at, \
                    COUNT(d.id), {covered} \
             FROM protection_defence_events d \
             JOIN tracking_sessions s ON s.id = d.session_id \
             LEFT JOIN session_definitions def ON def.id = s.definition_id \
             WHERE {unmeasured} \
             GROUP BY s.id \
             HAVING MAX(d.id) <= ?1 \
             ORDER BY s.started_at DESC, s.id DESC \
             LIMIT ?2"
        );
        let mut stmt = conn.prepare(&earlier_sql)?;
        let mut earlier = stmt
            .query_map(
                rusqlite::params![position.cursor, EARLIER_SESSIONS],
                candidate_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        earlier.reverse();
        earlier
    } else {
        Vec::new()
    };

    Ok(RecordingCandidates {
        stream,
        since: position.since,
        baseline_tt_ped,
        baseline_source,
        incomplete_account_reading_at: position.incomplete_reading_at,
        billed_from_account_ped: billed.tt_ped,
        account_tolerance_ped: billed.tolerance_ped,
        measured_sessions,
        sessions,
        earlier,
    })
}

/// How far behind one stream is: when it was last recorded, and the
/// sessions (and their hits) the readings did not measure played since.
pub(super) fn read_backlog(
    conn: &rusqlite::Connection,
    stream: ProtectionStream,
    readings: &mut ReadingCache,
) -> Result<StreamBacklog, ProtectionError> {
    let position = stream_position(conn, stream, readings)?;
    if matches!(stream, ProtectionStream::Limited { .. }) && position.baseline.is_none() {
        return Ok(StreamBacklog::default());
    }
    // id-order: cursor (hits on the far side of the stream's position).
    let (sessions, hits) = conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT d.session_id), COUNT(d.id) \
             FROM protection_defence_events d \
             JOIN tracking_sessions s ON s.id = d.session_id \
             WHERE d.id > ?1 AND NOT {}",
            armour_measured_sql(measured_stream(stream), "s.id")
        ),
        [position.cursor],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(StreamBacklog {
        last_recorded_at: position.since,
        sessions,
        hits,
    })
}

/// Why a recording was refused before anything was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    UnknownSession,
    SessionBeforeBaseline,
    SessionWithoutHits,
    /// The readings measured the chosen session's share: it is billed.
    SessionMeasured,
    /// Every session since the previous recording was measured, so the
    /// recording has nothing to cover.
    AllMeasured(ProtectionStream),
    /// A repair below the decay the readings already billed since the last
    /// one: it cannot have restored that decay.
    RepairBelowBilled,
    /// A repair that restored only decay the readings already billed.
    RepairAllBilled,
    /// A reading measuring less loss than the sessions the readings billed
    /// since the baseline lost.
    ReadingBelowBilled,
    ReadingIncreased,
}

impl Refusal {
    fn into_error(self) -> ProtectionError {
        match self {
            Self::UnknownSession => ProtectionError::Invalid("A chosen session no longer exists"),
            Self::SessionBeforeBaseline => ProtectionError::Invalid(
                "A chosen session was played before this set's previous reading",
            ),
            Self::SessionWithoutHits => {
                ProtectionError::Invalid("A chosen session has no recorded hits to weigh")
            }
            Self::SessionMeasured => {
                ProtectionError::Invalid("A chosen session's armour was measured from your account")
            }
            Self::RepairBelowBilled => ProtectionError::Invalid(
                "This repair is less than the decay already billed from your account since the last repair",
            ),
            Self::RepairAllBilled => ProtectionError::Invalid(
                "Everything this repair restored was already billed from your account",
            ),
            Self::ReadingBelowBilled => ProtectionError::Invalid(
                "This set lost less since its baseline than the sessions already billed from your account",
            ),
            Self::AllMeasured(ProtectionStream::Unlimited) => ProtectionError::Invalid(
                "Every session since the last repair was measured from your account",
            ),
            Self::AllMeasured(ProtectionStream::Limited { .. }) => ProtectionError::Invalid(
                "Every session since this set's last reading was measured from your account",
            ),
            Self::ReadingIncreased => {
                ProtectionError::Conflict("TT value increased; reset the baseline instead")
            }
        }
    }
}

/// Refuse a recording that would cover nothing because the readings
/// measured every session since the stream's previous recording: none
/// chosen, none left to choose, and at least one measured.
fn refuse_all_measured(
    conn: &rusqlite::Connection,
    stream: ProtectionStream,
    position: &StreamPosition,
    sessions: &[String],
) -> Result<Option<Refusal>, rusqlite::Error> {
    if !sessions.is_empty() || unmeasured_since(conn, stream, position)? > 0 {
        return Ok(None);
    }
    Ok((measured_since(conn, stream, position)? > 0).then_some(Refusal::AllMeasured(stream)))
}

/// One cost about to be written and spread.
struct Recording {
    kind: CostKind,
    stream: ProtectionStream,
    opening_observation_id: Option<i64>,
    closing_observation_id: Option<i64>,
    consumed_tt_ped: Option<f64>,
    markup_percent: Option<f64>,
    cost_ped: f64,
    client_token: Option<String>,
    created_at: f64,
    stream_cursor: i64,
    evidence_cursor: i64,
}

/// What a recording leaves to spread once what the readings already billed
/// is taken off: `measured` less `billed`, nothing when that is within the
/// readings' tolerance of nothing, and refused with `below` when it is
/// further below.
fn left_to_spread(measured: f64, billed: Billed, below: Refusal) -> Result<f64, Refusal> {
    let left = measured - billed.tt_ped;
    if left < -(billed.tolerance_ped + 0.000_000_1) {
        Err(below)
    } else {
        Ok(left.max(0.0))
    }
}

/// Hits per (session, context key) that one recording weighs.
type ContextHits = BTreeMap<(String, i64), (Option<i64>, i64)>;

fn weigh_sessions(
    tx: &rusqlite::Transaction<'_>,
    recording: &Recording,
    sessions: &[String],
) -> Result<Result<ContextHits, Refusal>, DbError> {
    let mut contexts = ContextHits::new();
    let chosen: BTreeSet<&String> = sessions.iter().collect();
    for session_id in chosen {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM tracking_sessions WHERE id = ?1)",
            [session_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(Err(Refusal::UnknownSession));
        }
        let measured: bool = tx.query_row(
            &format!(
                "SELECT {}",
                armour_measured_sql(measured_stream(recording.stream), "?1")
            ),
            [session_id],
            |row| row.get(0),
        )?;
        if measured {
            return Ok(Err(Refusal::SessionMeasured));
        }
        // id-order: cursor (hits between the stream position and now).
        let fresh: i64 = tx.query_row(
            "SELECT COUNT(*) FROM protection_defence_events \
             WHERE session_id = ?1 AND id > ?2 AND id <= ?3",
            rusqlite::params![
                session_id,
                recording.stream_cursor,
                recording.evidence_cursor
            ],
            |row| row.get(0),
        )?;
        let lower = if fresh > 0 {
            recording.stream_cursor
        } else if matches!(recording.stream, ProtectionStream::Limited { .. }) {
            return Ok(Err(Refusal::SessionBeforeBaseline));
        } else {
            0
        };
        // id-order: cursor (the session's hits inside the weighed range).
        let mut stmt = tx.prepare(
            "SELECT context_id, COUNT(*) FROM protection_defence_events \
             WHERE session_id = ?1 AND id > ?2 AND id <= ?3 \
             GROUP BY context_id",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![session_id, lower, recording.evidence_cursor],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() {
            return Ok(Err(Refusal::SessionWithoutHits));
        }
        for (context_id, hits) in rows {
            contexts.insert(
                (session_id.clone(), context_id.unwrap_or(-1)),
                (context_id, hits),
            );
        }
    }
    Ok(Ok(contexts))
}

/// Write one recording and spread it over the chosen sessions, repairing
/// every derived figure in the same transaction.
fn write_recording(
    tx: &rusqlite::Transaction<'_>,
    recording: Recording,
    sessions: &[String],
) -> Result<Result<i64, Refusal>, DbError> {
    let contexts = match weigh_sessions(tx, &recording, sessions)? {
        Ok(contexts) => contexts,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let (status, reason) = if contexts.is_empty() {
        ("pending", Some(UNATTRIBUTED_REASON))
    } else {
        ("booked", None)
    };
    let set_id = match recording.stream {
        ProtectionStream::Limited { set_id } => Some(set_id),
        ProtectionStream::Unlimited => None,
    };
    tx.execute(
        "INSERT INTO protection_cost_windows \
         (kind, set_id, opening_observation_id, closing_observation_id, consumed_tt_ped, \
          markup_percent, cost_ped, status, reason, client_token, created_at, evidence_cursor) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            recording.kind.as_str(),
            set_id,
            recording.opening_observation_id,
            recording.closing_observation_id,
            recording.consumed_tt_ped,
            recording.markup_percent,
            recording.cost_ped,
            status,
            reason,
            recording.client_token,
            recording.created_at,
            recording.evidence_cursor,
        ],
    )?;
    let window_id = tx.last_insert_rowid();

    let total_hits: i64 = contexts.values().map(|(_, hits)| hits).sum();
    let context_count = contexts.len();
    let mut allocated = 0.0;
    let mut per_session: BTreeMap<String, (i64, f64, f64)> = BTreeMap::new();
    for (index, ((session_id, context_key), (context_id, hits))) in contexts.into_iter().enumerate()
    {
        let share = hits as f64 / total_hits as f64;
        // The last context takes the residual, so the stored allocations
        // sum to the recorded cost exactly whatever the rounding.
        let cost = if index + 1 == context_count {
            (recording.cost_ped - allocated).max(0.0)
        } else {
            recording.cost_ped * share
        };
        allocated += cost;
        tx.execute(
            "INSERT INTO protection_cost_context_allocations \
             (window_id, session_id, context_key, context_id, damage_weight, deflection_count, \
              allocation_share, cost_ped, hit_count) \
             VALUES (?1, ?2, ?3, ?4, 0, 0, ?5, ?6, ?7)",
            rusqlite::params![
                window_id,
                session_id,
                context_key,
                context_id,
                share,
                cost,
                hits
            ],
        )?;
        let session = per_session.entry(session_id).or_default();
        session.0 += hits;
        session.1 += share;
        session.2 += cost;
    }
    for (session_id, (hits, share, cost)) in per_session {
        tx.execute(
            "INSERT INTO protection_cost_allocations \
             (window_id, session_id, damage_weight, deflection_count, allocation_share, \
              cost_ped, hit_count) \
             VALUES (?1, ?2, 0, 0, ?3, ?4, ?5)",
            rusqlite::params![window_id, session_id, share, cost, hits],
        )?;
        adjust_session_armour(tx, &session_id, cost)?;
    }
    Ok(Ok(window_id))
}

/// Move one session's armour cost by `delta` and repair every figure
/// derived from it (its summary and its day's rollup) in the caller's
/// transaction. A reversal that lands within rounding of zero stores zero,
/// so undoing the only recording leaves no residue behind.
pub(super) fn adjust_session_armour(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    delta: f64,
) -> Result<(), DbError> {
    let exists: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM tracking_sessions WHERE id = ?1)",
        [session_id],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    tx.execute(
        "UPDATE tracking_sessions SET armour_cost = \
             CASE WHEN abs(COALESCE(armour_cost, 0) + ?1) < 1e-9 THEN 0 \
                  ELSE COALESCE(armour_cost, 0) + ?1 END \
         WHERE id = ?2",
        rusqlite::params![delta, session_id],
    )?;
    // The resolution first, then the summary, days and rollup cells, so
    // every figure reads the session's armour as it now resolves.
    crate::session_costs::refresh_derived(tx, session_id)?;
    Ok(())
}

/// Keep the account reading a limited set's loss was measured against as
/// a reading of the set: its pieces' summed TT, dated by the account
/// reading, at the stream's position, and linked to the account reading.
fn account_reading_observation(
    tx: &rusqlite::Transaction<'_>,
    set_id: i64,
    closing_token: &str,
    tt_value_ped: f64,
    observed_at: f64,
    cursor: i64,
    reading_id: i64,
) -> Result<i64, DbError> {
    tx.execute(
        "INSERT INTO protection_observations \
         (set_id, client_token, tt_value_ped, source, raw_text, observed_at, reset_reason, \
          defence_event_cursor) \
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, NULL, ?6)",
        rusqlite::params![
            set_id,
            format!("account:{closing_token}"),
            tt_value_ped,
            ObservationSource::Account.as_str(),
            observed_at,
            cursor
        ],
    )?;
    let observation_id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO protection_observation_readings (observation_id, account_reading_id) \
         VALUES (?1, ?2)",
        rusqlite::params![observation_id, reading_id],
    )?;
    Ok(observation_id)
}

enum Written<T> {
    Saved(T),
    /// A confirmation repeated with its token: nothing was written.
    Replayed(T),
    Refused(Refusal),
}

fn read_observation_outcome(
    conn: &rusqlite::Connection,
    observation_id: i64,
) -> Result<ObservationOutcome, ProtectionError> {
    let observation = read_observation(conn, observation_id)?;
    let window_id = conn
        .query_row(
            "SELECT id FROM protection_cost_windows \
             WHERE closing_observation_id = ?1 AND superseded_at IS NULL",
            [observation_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let cost_window = window_id.map(|id| read_cost_window(conn, id)).transpose()?;
    Ok(ObservationOutcome {
        observation,
        cost_window,
    })
}

impl ProtectionService {
    /// Confirm a Trade Terminal reading of one limited set.
    ///
    /// The first reading, or one that resets the baseline, books nothing.
    /// A later reading books the TT lost since the previous one at the
    /// set's markup and spreads it over `session_ids`. A reading above the
    /// baseline is refused unless it says why the baseline moved.
    #[allow(clippy::too_many_arguments)]
    pub async fn confirm_observation(
        &self,
        set_id: i64,
        client_token: &str,
        tt_value_ped: f64,
        source: ObservationSource,
        raw_text: Option<&str>,
        reset_reason: Option<&str>,
        session_ids: Vec<String>,
    ) -> Result<ObservationOutcome, ProtectionError> {
        let token = client_token.trim().to_string();
        if token.is_empty() {
            return Err(ProtectionError::Invalid("Observation token is required"));
        }
        if !tt_value_ped.is_finite() || tt_value_ped < 0.0 {
            return Err(ProtectionError::Invalid("TT value must be zero or greater"));
        }
        let set = self.active_set(set_id).await?;
        let markup = set.markup_percent;
        let now = self.now();
        let game_data = self.game_data.clone();
        let source_text = source.as_str();
        let raw_text = raw_text
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        let reset_reason = reset_reason
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map(str::to_string);

        let written = self
            .db
            .with_writer(move |conn| {
                if let Some(existing) = conn
                    .query_row(
                        "SELECT id FROM protection_observations WHERE client_token = ?1",
                        [&token],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                {
                    return Ok(Written::Replayed(existing));
                }
                let stream = ProtectionStream::Limited { set_id };
                let tx = conn.transaction()?;
                let position = stream_position(&tx, stream, &mut ReadingCache::default())
                    .map_err(super::protection_decode)?;
                let roles = KnownRoles::read(&tx, game_data.as_deref())
                    .map_err(account_error)
                    .map_err(super::protection_decode)?;
                let billed = billed_from_account(&tx, stream, &position, &roles)
                    .map_err(super::protection_decode)?;
                if reset_reason.is_none()
                    && position.baseline.as_ref().is_some_and(|b| {
                        tt_value_ped > b.tt_value_ped + billed.tolerance_ped + 0.000_000_1
                    })
                {
                    return Ok(Written::Refused(Refusal::ReadingIncreased));
                }
                // The loss to spread: the baseline's fall to this reading,
                // less what the sessions the readings measured since already
                // billed of it.
                let mut consumed = 0.0;
                if let (None, Some(baseline)) = (&reset_reason, &position.baseline) {
                    consumed = match left_to_spread(
                        (baseline.tt_value_ped - tt_value_ped).max(0.0),
                        billed,
                        Refusal::ReadingBelowBilled,
                    ) {
                        Ok(left) => left,
                        Err(refusal) => return Ok(Written::Refused(refusal)),
                    };
                    if let Some(refusal) =
                        refuse_all_measured(&tx, stream, &position, &session_ids)?
                    {
                        return Ok(Written::Refused(refusal));
                    }
                }
                let evidence_cursor = current_cursor(&tx)?;
                tx.execute(
                    "INSERT INTO protection_observations \
                     (set_id, client_token, tt_value_ped, source, raw_text, observed_at, \
                      reset_reason, defence_event_cursor) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        set_id,
                        token,
                        tt_value_ped,
                        source_text,
                        raw_text,
                        now,
                        reset_reason.as_deref(),
                        evidence_cursor
                    ],
                )?;
                let observation_id = tx.last_insert_rowid();

                if let (None, Some(baseline)) = (&reset_reason, &position.baseline) {
                    let opening = match baseline.origin {
                        BaselineOrigin::Observation(id) => id,
                        BaselineOrigin::Account { reading_id, .. } => account_reading_observation(
                            &tx,
                            set_id,
                            &token,
                            baseline.tt_value_ped,
                            position.since.unwrap_or(now),
                            position.cursor,
                            reading_id,
                        )?,
                    };
                    let recording = Recording {
                        kind: CostKind::LimitedDecay,
                        stream,
                        opening_observation_id: Some(opening),
                        closing_observation_id: Some(observation_id),
                        consumed_tt_ped: Some(consumed),
                        markup_percent: Some(markup),
                        cost_ped: consumed * markup / 100.0,
                        client_token: None,
                        created_at: now,
                        stream_cursor: position.cursor,
                        evidence_cursor,
                    };
                    if let Err(refusal) = write_recording(&tx, recording, &session_ids)? {
                        return Ok(Written::Refused(refusal));
                    }
                }
                tx.commit()?;
                Ok(Written::Saved(observation_id))
            })
            .await?;
        match written {
            Written::Saved(observation_id) | Written::Replayed(observation_id) => {
                if matches!(written, Written::Saved(_)) {
                    self.notify_changed();
                }
                self.db
                    .with_reader(move |conn| {
                        read_observation_outcome(conn, observation_id)
                            .map_err(super::protection_decode)
                    })
                    .await
                    .map_err(ProtectionError::from)
            }
            Written::Refused(refusal) => Err(refusal.into_error()),
        }
    }

    /// Confirm an unlimited repair total and spread it over `session_ids`.
    /// Repeating a confirmation with the same token returns the first.
    pub async fn confirm_repair_cost(
        &self,
        client_token: &str,
        cost_ped: f64,
        session_ids: Vec<String>,
    ) -> Result<RepairOutcome, ProtectionError> {
        let token = client_token.trim();
        if token.is_empty() {
            return Err(ProtectionError::Invalid("Repair token is required"));
        }
        if !cost_ped.is_finite() || cost_ped < 0.0 {
            return Err(ProtectionError::Invalid(
                "Repair cost must be zero or greater",
            ));
        }
        let token = format!("repair:{token}");
        let now = self.now();
        let game_data = self.game_data.clone();
        let written = self
            .db
            .with_writer(move |conn| {
                if let Some(id) = conn
                    .query_row(
                        "SELECT id FROM protection_cost_windows \
                         WHERE client_token = ?1 AND kind = 'repair'",
                        [&token],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                {
                    return Ok(Written::Replayed(id));
                }
                let tx = conn.transaction()?;
                let stream = ProtectionStream::Unlimited;
                let position = stream_position(&tx, stream, &mut ReadingCache::default())
                    .map_err(super::protection_decode)?;
                let roles = KnownRoles::read(&tx, game_data.as_deref())
                    .map_err(account_error)
                    .map_err(super::protection_decode)?;
                let billed = billed_from_account(&tx, stream, &position, &roles)
                    .map_err(super::protection_decode)?;
                // The repair restored the decay the readings already billed
                // too: only the rest is this recording's to spread.
                let mut spread = cost_ped;
                if billed.tt_ped > 0.0 {
                    spread = match left_to_spread(cost_ped, billed, Refusal::RepairBelowBilled) {
                        Ok(left) if left > billed.tolerance_ped + 0.000_000_1 => left,
                        Ok(_) => return Ok(Written::Refused(Refusal::RepairAllBilled)),
                        Err(refusal) => return Ok(Written::Refused(refusal)),
                    };
                }
                if let Some(refusal) = refuse_all_measured(&tx, stream, &position, &session_ids)? {
                    return Ok(Written::Refused(refusal));
                }
                let recording = Recording {
                    kind: CostKind::Repair,
                    stream: ProtectionStream::Unlimited,
                    opening_observation_id: None,
                    closing_observation_id: None,
                    consumed_tt_ped: None,
                    markup_percent: None,
                    cost_ped: spread,
                    client_token: Some(token.clone()),
                    created_at: now,
                    stream_cursor: position.cursor,
                    evidence_cursor: current_cursor(&tx)?,
                };
                let id = match write_recording(&tx, recording, &session_ids)? {
                    Ok(id) => id,
                    Err(refusal) => return Ok(Written::Refused(refusal)),
                };
                tx.commit()?;
                Ok(Written::Saved(id))
            })
            .await?;
        match written {
            Written::Saved(id) | Written::Replayed(id) => {
                if matches!(written, Written::Saved(_)) {
                    self.notify_changed();
                }
                let cost_window = self
                    .db
                    .with_reader(move |conn| {
                        read_cost_window(conn, id).map_err(super::protection_decode)
                    })
                    .await?;
                Ok(RepairOutcome { cost_window })
            }
            Written::Refused(refusal) => Err(refusal.into_error()),
        }
    }
}
