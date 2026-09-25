//! Recording a protection cost and spreading it over sessions.
//!
//! Every stream keeps a position on the defence-event stream: the highest
//! hit id its previous recording reached (for a limited set, the cursor its
//! current baseline reading was taken at). A new recording weighs each
//! ticked session by its hits past that position, up to the hits recorded
//! when it is confirmed. A session entirely before the position can still
//! be re-included in an unlimited recording (a piece left unrepaired last
//! time); it is then weighed by all of its hits. A limited reading cannot
//! reach back past its baseline, because the TT it measured was lost after
//! the baseline was taken.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::OptionalExtension;

use super::read::{read_cost_window, read_observation};
use super::{
    CandidateSession, CostKind, ObservationOutcome, ObservationSource, ProtectionError,
    ProtectionService, ProtectionStream, RecordingCandidates, RepairOutcome,
};
use crate::db::DbError;

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

/// Where one stream's previous recording left the defence-event stream.
struct StreamPosition {
    cursor: i64,
    since: Option<f64>,
    baseline: Option<Baseline>,
}

struct Baseline {
    observation_id: i64,
    tt_value_ped: f64,
}

fn stream_position(
    conn: &rusqlite::Connection,
    stream: ProtectionStream,
) -> Result<StreamPosition, rusqlite::Error> {
    match stream {
        ProtectionStream::Unlimited => {
            let (cursor, since) = conn.query_row(
                "SELECT COALESCE(MAX(evidence_cursor), 0), MAX(created_at) \
                 FROM protection_cost_windows WHERE kind = 'repair'",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<f64>>(1)?)),
            )?;
            Ok(StreamPosition {
                cursor,
                since,
                baseline: None,
            })
        }
        ProtectionStream::Limited { set_id } => {
            let latest = conn
                .query_row(
                    "SELECT id, tt_value_ped, observed_at, COALESCE(defence_event_cursor, 0) \
                     FROM protection_observations WHERE set_id = ?1 \
                     ORDER BY observed_at DESC, id DESC LIMIT 1",
                    [set_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, f64>(1)?,
                            row.get::<_, f64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
                .optional()?;
            Ok(match latest {
                Some((observation_id, tt_value_ped, observed_at, cursor)) => StreamPosition {
                    cursor,
                    since: Some(observed_at),
                    baseline: Some(Baseline {
                        observation_id,
                        tt_value_ped,
                    }),
                },
                None => StreamPosition {
                    cursor: 0,
                    since: None,
                    baseline: None,
                },
            })
        }
    }
}

/// The newest recorded hit: a recording covers every hit up to here.
fn current_cursor(conn: &rusqlite::Connection) -> Result<i64, rusqlite::Error> {
    // id-order: cursor (the highest defence-event id seen so far).
    conn.query_row(
        "SELECT COALESCE(MAX(id), 0) FROM protection_defence_events",
        [],
        |row| row.get(0),
    )
}

/// The stream's own windows, as a SQL predicate over `w`.
fn stream_filter(stream: ProtectionStream) -> String {
    match stream {
        ProtectionStream::Unlimited => "w.kind = 'repair'".to_string(),
        ProtectionStream::Limited { set_id } => {
            format!("w.kind = 'limited_decay' AND w.set_id = {set_id}")
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
    })
}

pub(super) fn read_candidates(
    conn: &rusqlite::Connection,
    stream: ProtectionStream,
) -> Result<RecordingCandidates, ProtectionError> {
    let position = stream_position(conn, stream)?;
    let baseline_tt_ped = position.baseline.as_ref().map(|b| b.tt_value_ped);
    if matches!(stream, ProtectionStream::Limited { .. }) && position.baseline.is_none() {
        // A first reading is a baseline: there is nothing to spread yet.
        return Ok(RecordingCandidates {
            stream,
            since: None,
            baseline_tt_ped,
            sessions: Vec::new(),
            earlier: Vec::new(),
        });
    }
    let covered = format!(
        "EXISTS (SELECT 1 FROM protection_cost_allocations a \
                 JOIN protection_cost_windows w ON w.id = a.window_id \
                 WHERE a.session_id = s.id AND {})",
        stream_filter(stream)
    );

    // id-order: cursor (hits on the far side of the stream's position).
    let fresh_sql = format!(
        "SELECT s.id, s.session_name, s.definition_id, def.name, s.started_at, s.ended_at, \
                COUNT(d.id), {covered} \
         FROM protection_defence_events d \
         JOIN tracking_sessions s ON s.id = d.session_id \
         LEFT JOIN session_definitions def ON def.id = s.definition_id \
         WHERE d.id > ?1 \
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
        sessions,
        earlier,
    })
}

/// Why a recording was refused before anything was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    UnknownSession,
    SessionBeforeBaseline,
    SessionWithoutHits,
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
            Self::ReadingIncreased => {
                ProtectionError::Conflict("TT value increased; reset the baseline instead")
            }
        }
    }
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
        let started_at: f64 = tx.query_row(
            "SELECT started_at FROM tracking_sessions WHERE id = ?1",
            [&session_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE tracking_sessions SET armour_cost = COALESCE(armour_cost, 0) + ?1 \
             WHERE id = ?2",
            rusqlite::params![cost, session_id],
        )?;
        crate::daily_rollup::refresh_days(tx, [crate::daily_rollup::epoch_day(started_at)])?;
        crate::session_summary::write_session_summary(tx, &session_id)?;
    }
    Ok(Ok(window_id))
}

enum Written<T> {
    Saved(T),
    Refused(Refusal),
}

fn read_observation_outcome(
    conn: &rusqlite::Connection,
    observation_id: i64,
) -> Result<ObservationOutcome, ProtectionError> {
    let observation = read_observation(conn, observation_id)?;
    let window_id = conn
        .query_row(
            "SELECT id FROM protection_cost_windows WHERE closing_observation_id = ?1",
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
                    return Ok(Written::Saved(existing));
                }
                let stream = ProtectionStream::Limited { set_id };
                let position = stream_position(conn, stream)?;
                if reset_reason.is_none()
                    && position
                        .baseline
                        .as_ref()
                        .is_some_and(|b| tt_value_ped > b.tt_value_ped + 0.000_000_1)
                {
                    return Ok(Written::Refused(Refusal::ReadingIncreased));
                }

                let tx = conn.transaction()?;
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
                    let consumed = (baseline.tt_value_ped - tt_value_ped).max(0.0);
                    let recording = Recording {
                        kind: CostKind::LimitedDecay,
                        stream,
                        opening_observation_id: Some(baseline.observation_id),
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
            Written::Saved(observation_id) => self
                .db
                .with_reader(move |conn| {
                    read_observation_outcome(conn, observation_id).map_err(super::protection_decode)
                })
                .await
                .map_err(ProtectionError::from),
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
                    return Ok(Written::Saved(id));
                }
                let tx = conn.transaction()?;
                let position = stream_position(&tx, ProtectionStream::Unlimited)?;
                let recording = Recording {
                    kind: CostKind::Repair,
                    stream: ProtectionStream::Unlimited,
                    opening_observation_id: None,
                    closing_observation_id: None,
                    consumed_tt_ped: None,
                    markup_percent: None,
                    cost_ped,
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
            Written::Saved(id) => {
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
