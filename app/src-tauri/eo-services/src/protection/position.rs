//! Where a new recording of one stream starts, and what of it the account
//! readings already billed.
//!
//! Every stream keeps a position on the defence-event stream: the highest
//! hit id its previous recording reached (for a limited set, the cursor its
//! current baseline was taken at). A session whose share of the stream the
//! readings measured is already billed and is never covered, but it moves
//! no position: every session the readings did not measure stays coverable,
//! whenever it was played.
//!
//! A limited set measures a Trade Terminal reading against a baseline: its
//! own last reading, or, when newer, its pieces in the last account reading
//! taken at or before the first session since then the readings did not
//! measure, so that session's loss is inside the reading. That reading must
//! hold every confirmed piece: one that lacks a piece cannot say what the
//! set held, and the set needs a Trade Terminal reading as its baseline.
//!
//! What a recording measures can include loss the readings already billed:
//! a repair restores the decay of measured sessions too, and a limited
//! set's fall since its baseline includes what measured sessions lost.
//! That part is known exactly from the readings, and is taken off before
//! anything is spread.

use std::collections::{hash_map::Entry, BTreeSet, HashMap};

use rusqlite::OptionalExtension;

use super::{account_error, BaselineSource, ProtectionError, ProtectionStream};
use crate::account::reading::Reading;
use crate::account::store::{last_reading_by, load_reading, ReadingId};
use crate::account::KnownRoles;
use crate::session_costs::{armour_lost_sql, armour_measured_sql, ArmourStream};

/// The account readings a request has loaded, by id, so each is rebuilt
/// once however many streams ask for it.
#[derive(Default)]
pub(super) struct ReadingCache {
    loaded: HashMap<i64, Reading>,
}

impl ReadingCache {
    /// The last reading taken at or before `at` (of all, when absent).
    fn by(
        &mut self,
        conn: &rusqlite::Connection,
        at: Option<f64>,
    ) -> Result<Option<(i64, &Reading)>, ProtectionError> {
        let Some(ReadingId(id)) = last_reading_by(conn, at).map_err(account_error)? else {
            return Ok(None);
        };
        if let Entry::Vacant(slot) = self.loaded.entry(id) {
            let Some(stored) = load_reading(conn, ReadingId(id)).map_err(account_error)? else {
                return Ok(None);
            };
            slot.insert(stored.reading);
        }
        Ok(self.loaded.get(&id).map(|reading| (id, reading)))
    }
}

/// Where one stream's previous recording left the defence-event stream.
pub(super) struct StreamPosition {
    /// Where a new recording's look-back starts: the previous recording,
    /// or the baseline.
    pub(super) cursor: i64,
    /// Where the player's own previous recording left it.
    pub(super) recorded_cursor: i64,
    pub(super) since: Option<f64>,
    pub(super) baseline: Option<Baseline>,
    /// An account reading passed over as the baseline because it does not
    /// hold every confirmed piece: when it was taken.
    pub(super) incomplete_reading_at: Option<f64>,
    /// How many pieces the set has confirmed (none for unlimited armour).
    pub(super) pieces: usize,
}

pub(super) struct Baseline {
    pub(super) origin: BaselineOrigin,
    pub(super) tt_value_ped: f64,
}

/// Where a limited set's baseline was read.
pub(super) enum BaselineOrigin {
    /// The set's own reading.
    Observation(i64),
    /// An account reading holding every confirmed piece.
    Account { reading_id: i64 },
}

impl Baseline {
    pub(super) fn source(&self) -> BaselineSource {
        match self.origin {
            BaselineOrigin::Observation(_) => BaselineSource::TradeTerminal,
            BaselineOrigin::Account { .. } => BaselineSource::Account,
        }
    }
}

/// The armour stream the readings measure for a protection stream.
pub(super) fn measured_stream(stream: ProtectionStream) -> ArmourStream {
    match stream {
        ProtectionStream::Unlimited => ArmourStream::Unlimited,
        ProtectionStream::Limited { set_id } => ArmourStream::Set(set_id),
    }
}

/// The last hit of every session that had ended by `at`.
fn cursor_at(conn: &rusqlite::Connection, at: f64) -> Result<i64, rusqlite::Error> {
    // id-order: cursor (the highest hit of the sessions ended by then).
    conn.query_row(
        "SELECT COALESCE(MAX(d.id), 0) FROM protection_defence_events d \
         JOIN tracking_sessions s ON s.id = d.session_id \
         WHERE s.ended_at IS NOT NULL AND s.ended_at <= ?1",
        [at],
        |row| row.get(0),
    )
}

/// When the first session with hits past `cursor` that the readings did
/// not measure for `stream` started.
fn first_unmeasured_since(
    conn: &rusqlite::Connection,
    stream: ProtectionStream,
    cursor: i64,
) -> Result<Option<f64>, rusqlite::Error> {
    // id-order: cursor (hits on the far side of the position).
    conn.query_row(
        &format!(
            "SELECT MIN(s.started_at) FROM protection_defence_events d \
             JOIN tracking_sessions s ON s.id = d.session_id \
             WHERE d.id > ?1 AND NOT {}",
            armour_measured_sql(measured_stream(stream), "s.id")
        ),
        [cursor],
        |row| row.get(0),
    )
}

pub(super) fn stream_position(
    conn: &rusqlite::Connection,
    stream: ProtectionStream,
    readings: &mut ReadingCache,
) -> Result<StreamPosition, ProtectionError> {
    match stream {
        ProtectionStream::Unlimited => {
            let (recorded_cursor, since) = conn.query_row(
                "SELECT COALESCE(MAX(evidence_cursor), 0), MAX(created_at) \
                 FROM protection_cost_windows \
                 WHERE kind = 'repair' AND superseded_at IS NULL",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<f64>>(1)?)),
            )?;
            Ok(StreamPosition {
                cursor: recorded_cursor,
                recorded_cursor,
                since,
                baseline: None,
                incomplete_reading_at: None,
                pieces: 0,
            })
        }
        ProtectionStream::Limited { set_id } => {
            let latest = conn
                .query_row(
                    "SELECT id, tt_value_ped, observed_at, COALESCE(defence_event_cursor, 0) \
                     FROM protection_observations \
                     WHERE set_id = ?1 AND superseded_at IS NULL \
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
            let recorded_cursor = latest.map_or(0, |(_, _, _, cursor)| cursor);
            let pieces = super::pieces::confirmed_names(conn, set_id)?;
            let mut incomplete_reading_at = None;
            let mut from_account = None;
            if !pieces.is_empty() {
                let first = first_unmeasured_since(conn, stream, recorded_cursor)?;
                if let Some((reading_id, reading)) = readings.by(conn, first)? {
                    let newer = latest
                        .is_none_or(|(_, _, observed_at, _)| reading.taken_at() > observed_at);
                    if newer {
                        let mut held = BTreeSet::new();
                        let mut pec: i64 = 0;
                        for (key, holding) in reading.holdings() {
                            if pieces.contains(&key.name) {
                                held.insert(key.name.as_str());
                                pec = pec.checked_add(holding.value().0).ok_or(
                                    ProtectionError::Stored(
                                        "a set's pieces are worth more than can be counted",
                                    ),
                                )?;
                            }
                        }
                        if held.len() == pieces.len() {
                            from_account =
                                Some((reading_id, reading.taken_at(), pec as f64 / 100.0));
                        } else {
                            incomplete_reading_at = Some(reading.taken_at());
                        }
                    }
                }
            }
            let (cursor, since, baseline) = match (from_account, latest) {
                (Some((reading_id, taken_at, tt_value_ped)), _) => (
                    cursor_at(conn, taken_at)?,
                    Some(taken_at),
                    Some(Baseline {
                        origin: BaselineOrigin::Account { reading_id },
                        tt_value_ped,
                    }),
                ),
                (None, Some((observation_id, tt_value_ped, observed_at, cursor))) => (
                    cursor,
                    Some(observed_at),
                    Some(Baseline {
                        origin: BaselineOrigin::Observation(observation_id),
                        tt_value_ped,
                    }),
                ),
                (None, None) => (0, None, None),
            };
            Ok(StreamPosition {
                cursor,
                recorded_cursor,
                since,
                baseline,
                incomplete_reading_at,
                pieces: pieces.len(),
            })
        }
    }
}

/// How many sessions the readings measured for `stream` since the
/// player's own previous recording of it.
pub(super) fn measured_since(
    conn: &rusqlite::Connection,
    stream: ProtectionStream,
    position: &StreamPosition,
) -> Result<i64, rusqlite::Error> {
    // id-order: cursor (hits on the far side of the previous recording).
    conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT d.session_id) FROM protection_defence_events d \
             WHERE d.id > ?1 AND {}",
            armour_measured_sql(measured_stream(stream), "d.session_id")
        ),
        [position.recorded_cursor],
        |row| row.get(0),
    )
}

/// How many sessions the readings did not measure for `stream` have hits
/// past its position: what a new recording can cover.
pub(super) fn unmeasured_since(
    conn: &rusqlite::Connection,
    stream: ProtectionStream,
    position: &StreamPosition,
) -> Result<i64, rusqlite::Error> {
    // id-order: cursor (hits on the far side of the stream's position).
    conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT d.session_id) FROM protection_defence_events d \
             WHERE d.id > ?1 AND NOT {}",
            armour_measured_sql(measured_stream(stream), "d.session_id")
        ),
        [position.cursor],
        |row| row.get(0),
    )
}

/// What the readings already billed of what a recording of `stream` would
/// measure, in PED of TT, and how far that figure may stray (a PEC per
/// piece it was read from).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Billed {
    pub(super) tt_ped: f64,
    pub(super) tolerance_ped: f64,
}

/// For unlimited armour, the decay of the sessions measured since the
/// previous recorded repair that no repair has restored since; for a
/// limited set, what its pieces lost in the sessions measured since its
/// baseline.
pub(super) fn billed_from_account(
    conn: &rusqlite::Connection,
    stream: ProtectionStream,
    position: &StreamPosition,
    roles: &KnownRoles,
) -> Result<Billed, ProtectionError> {
    let measured = measured_stream(stream);
    match stream {
        ProtectionStream::Unlimited => {
            let sessions: Vec<String> = {
                // id-order: cursor (hits on the far side of the previous repair).
                let mut stmt = conn.prepare(&format!(
                    "SELECT DISTINCT d.session_id FROM protection_defence_events d \
                     WHERE d.id > ?1 AND {}",
                    armour_measured_sql(measured, "d.session_id")
                ))?;
                let rows = stmt
                    .query_map([position.recorded_cursor], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let outstanding =
                crate::account::settlement::outstanding_unlimited_decay(conn, roles, &sessions)
                    .map_err(account_error)?;
            Ok(Billed {
                tt_ped: outstanding.decay.micro_ped() as f64 / 1_000_000.0,
                tolerance_ped: outstanding.pieces as f64 * 0.01,
            })
        }
        ProtectionStream::Limited { .. } => {
            // id-order: cursor (hits on the far side of the baseline).
            let lost: i64 = conn.query_row(
                &format!(
                    "SELECT COALESCE(SUM({}), 0) FROM ( \
                         SELECT DISTINCT d.session_id AS id FROM protection_defence_events d \
                         WHERE d.id > ?1) s",
                    armour_lost_sql(measured, "s.id")
                ),
                [position.cursor],
                |row| row.get(0),
            )?;
            let from_readings = lost > 0
                || matches!(
                    position.baseline,
                    Some(Baseline {
                        origin: BaselineOrigin::Account { .. },
                        ..
                    })
                );
            Ok(Billed {
                tt_ped: lost as f64 / 1_000_000.0,
                tolerance_ped: if from_readings {
                    position.pieces as f64 * 0.01
                } else {
                    0.0
                },
            })
        }
    }
}
