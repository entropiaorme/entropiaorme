//! Account readings and session measures on disk.
//!
//! A reading is written as one row of `account_readings` and the absolute
//! state of its holdings in `account_reading_holdings`, in one transaction
//! on the writer. A keyframe stores every holding; any other reading
//! stores only the holdings whose state differs from the reading before
//! it, with an empty state for a holding that is gone, so a reading is
//! rebuilt from the keyframe before it and the readings between. A reading
//! is a keyframe when no reading precedes it, when the one before it is
//! more than ten minutes older, when it is taken on connecting, when it is
//! the first reading of a tracked session, or when
//! [`KEYFRAME_EVERY`] readings of changes already follow the last keyframe.
//!
//! The chain follows the order readings were stored, never their times:
//! the wall clock can step back (the repeated hour when summer time ends),
//! and no reading is refused for its time. "The reading before" is the one
//! stored before it. A session's readings are the ones tagged with it,
//! ordered as they were stored; only readings tagged with no session are
//! chosen by a session's time window.
//!
//! A session's measure is cached in `account_session_measures` with the
//! version of the rules that computed it, and recomputed from the stored
//! readings when that version is not the code's (older, or a later
//! build's), or while the session is not sealed.

use std::collections::BTreeMap;

use rusqlite::OptionalExtension;

use super::amount::Pec;
use super::measure::{
    measure_own_readings, measure_session_with_baseline, SessionMeasure, SessionWindow,
    MEASURE_VERSION,
};
use super::reading::{Container, ContainerKind, Holding, HoldingKey, Reading, Row};
use super::roles::RoleSource;
use super::AccountError;
use crate::db::Db;

/// A reading taken longer than this after the one before it is a keyframe.
pub const KEYFRAME_AFTER_SECONDS: f64 = 600.0;

/// At most this many readings of changes follow a keyframe, so rebuilding
/// any reading reads a bounded chain.
pub const KEYFRAME_EVERY: usize = 30;

/// Why a reading was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// On connecting to the account site.
    Connect,
    /// The first reading of a tracked session, taken because it started.
    Baseline,
    /// While a tracked session runs, and when it has stopped.
    Session,
    /// For a bounded run of play outside a tracked session.
    Run,
    /// At the player's request.
    Manual,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Connect => "connect",
            Purpose::Baseline => "baseline",
            Purpose::Session => "session",
            Purpose::Run => "run",
            Purpose::Manual => "manual",
        }
    }

    pub fn parse(value: &str) -> Option<Purpose> {
        match value {
            "connect" => Some(Purpose::Connect),
            "baseline" => Some(Purpose::Baseline),
            "session" => Some(Purpose::Session),
            "run" => Some(Purpose::Run),
            "manual" => Some(Purpose::Manual),
            _ => None,
        }
    }
}

/// A stored reading's id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReadingId(pub i64);

/// How a reading's holdings are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// Every holding.
    Key,
    /// Only what changed since the reading before it.
    Delta,
}

/// A stored reading's own row.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadingHeader {
    pub id: ReadingId,
    pub taken_at: f64,
    pub purpose: Purpose,
    pub session_id: Option<String>,
    pub balance: Pec,
    pub frame: Frame,
    /// The reading stored before it, when there is one.
    pub prev: Option<ReadingId>,
    pub row_count: usize,
}

/// A stored reading, rebuilt whole.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredReading {
    pub header: ReadingHeader,
    pub reading: Reading,
}

const HEADER_COLUMNS: &str =
    "id, taken_at, purpose, session_id, balance_pec, is_keyframe, prev_reading_id, row_count";

type RawHeader = (i64, f64, String, Option<String>, i64, i64, Option<i64>, i64);

fn raw_header(row: &rusqlite::Row) -> rusqlite::Result<RawHeader> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn decode_header(raw: RawHeader) -> Result<ReadingHeader, AccountError> {
    let (id, taken_at, purpose, session_id, balance, keyframe, prev, row_count) = raw;
    Ok(ReadingHeader {
        id: ReadingId(id),
        taken_at,
        purpose: Purpose::parse(&purpose).ok_or(AccountError::Stored("unknown reading purpose"))?,
        session_id,
        balance: Pec(balance),
        frame: match keyframe {
            1 => Frame::Key,
            0 => Frame::Delta,
            _ => return Err(AccountError::Stored("unknown reading frame")),
        },
        prev: prev.map(ReadingId),
        row_count: usize::try_from(row_count)
            .map_err(|_| AccountError::Stored("negative row count"))?,
    })
}

/// One stored reading's own row.
pub fn header(
    conn: &rusqlite::Connection,
    id: ReadingId,
) -> Result<Option<ReadingHeader>, AccountError> {
    conn.query_row(
        &format!("SELECT {HEADER_COLUMNS} FROM account_readings WHERE id = ?1"),
        [id.0],
        raw_header,
    )
    .optional()?
    .map(decode_header)
    .transpose()
}

/// The reading stored last: the one a new reading follows in the chain.
pub fn last_stored(conn: &rusqlite::Connection) -> Result<Option<ReadingHeader>, AccountError> {
    conn.query_row(
        // id-order: insertion
        &format!("SELECT {HEADER_COLUMNS} FROM account_readings ORDER BY id DESC LIMIT 1"),
        [],
        raw_header,
    )
    .optional()?
    .map(decode_header)
    .transpose()
}

/// The holding states one reading stored, an empty holding standing for a
/// holding that is gone.
fn stored_holdings(
    conn: &rusqlite::Connection,
    id: ReadingId,
) -> Result<Vec<(HoldingKey, Holding)>, AccountError> {
    let mut stmt = conn.prepare(
        "SELECT item_name, container_kind, container_name, quantity, value_pec, row_count, \
                rows_json \
         FROM account_reading_holdings WHERE reading_id = ?1",
    )?;
    let rows = stmt
        .query_map([id.0], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(
            |(name, kind, container_name, quantity, value, row_count, rows_json)| {
                let container = ContainerKind::parse(&kind)
                    .and_then(|kind| Container::from_parts(kind, &container_name))
                    .ok_or(AccountError::Stored("unknown container"))?;
                let rows: Vec<(i64, i64)> = serde_json::from_str(&rows_json)
                    .map_err(|_| AccountError::Stored("a holding's rows are not pairs"))?;
                let holding = Holding::new(
                    rows.into_iter()
                        .map(|(quantity, value)| Row {
                            quantity,
                            value: Pec(value),
                        })
                        .collect(),
                )?;
                let whole = holding
                    .rows()
                    .iter()
                    .all(|row| row.quantity >= 0 && row.value.0 >= 0);
                if !whole
                    || holding.quantity() != quantity
                    || holding.value() != Pec(value)
                    || i64::try_from(holding.row_count()).ok() != Some(row_count)
                {
                    return Err(AccountError::Stored(
                        "a holding's rows do not add up to its totals",
                    ));
                }
                Ok((HoldingKey { name, container }, holding))
            },
        )
        .collect()
}

/// The full holdings of a stored reading: its keyframe's, with every later
/// reading's changes applied up to it; and how many readings of changes
/// follow that keyframe up to and including this one.
fn state_at(
    conn: &rusqlite::Connection,
    reading: &ReadingHeader,
) -> Result<(BTreeMap<HoldingKey, Holding>, usize), AccountError> {
    let mut chain = vec![reading.id];
    let mut frame = reading.frame;
    let mut prev = reading.prev;
    while frame == Frame::Delta {
        let id = prev.ok_or(AccountError::Stored(
            "a reading of changes has none to follow",
        ))?;
        let earlier =
            header(conn, id)?.ok_or(AccountError::Stored("a reading's predecessor is missing"))?;
        chain.push(earlier.id);
        frame = earlier.frame;
        prev = earlier.prev;
    }
    let changes_since_keyframe = chain.len() - 1;
    let mut state = BTreeMap::new();
    for id in chain.into_iter().rev() {
        apply(&mut state, stored_holdings(conn, id)?);
    }
    Ok((state, changes_since_keyframe))
}

fn apply(state: &mut BTreeMap<HoldingKey, Holding>, changes: Vec<(HoldingKey, Holding)>) {
    for (key, holding) in changes {
        if holding.row_count() == 0 {
            state.remove(&key);
        } else {
            state.insert(key, holding);
        }
    }
}

fn rebuild(
    header: ReadingHeader,
    holdings: BTreeMap<HoldingKey, Holding>,
) -> Result<StoredReading, AccountError> {
    let reading = Reading::new(header.taken_at, header.balance, holdings)?;
    Ok(StoredReading { header, reading })
}

/// One stored reading, rebuilt whole.
pub fn load_reading(
    conn: &rusqlite::Connection,
    id: ReadingId,
) -> Result<Option<StoredReading>, AccountError> {
    let Some(header) = header(conn, id)? else {
        return Ok(None);
    };
    let (holdings, _) = state_at(conn, &header)?;
    rebuild(header, holdings).map(Some)
}

/// Store a reading in one transaction. `session_id` names the tracked
/// session it belongs to.
pub fn record_reading(
    conn: &mut rusqlite::Connection,
    reading: &Reading,
    purpose: Purpose,
    session_id: Option<&str>,
) -> Result<ReadingHeader, AccountError> {
    let tx = conn.transaction()?;
    let latest = last_stored(&tx)?;
    let first_of_session = match session_id {
        Some(session) => !tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM account_readings WHERE session_id = ?1)",
            [session],
            |row| row.get::<_, bool>(0),
        )?,
        None => false,
    };
    let mut before_state: Option<BTreeMap<HoldingKey, Holding>> = None;
    let mut frame = Frame::Key;
    if let Some(latest) = &latest {
        if purpose != Purpose::Connect
            && !first_of_session
            && reading.taken_at() - latest.taken_at <= KEYFRAME_AFTER_SECONDS
        {
            let (state, changes) = state_at(&tx, latest)?;
            if changes < KEYFRAME_EVERY {
                frame = Frame::Delta;
                before_state = Some(state);
            }
        }
    }
    let writes: Vec<(HoldingKey, Holding)> = match before_state {
        Some(before) => {
            let mut writes: Vec<(HoldingKey, Holding)> = reading
                .holdings()
                .iter()
                .filter(|(key, holding)| before.get(*key) != Some(*holding))
                .map(|(key, holding)| (key.clone(), holding.clone()))
                .collect();
            writes.extend(
                before
                    .into_keys()
                    .filter(|key| reading.holding(key).is_none())
                    .map(|key| (key, Holding::default())),
            );
            writes
        }
        None => reading
            .holdings()
            .iter()
            .map(|(key, holding)| (key.clone(), holding.clone()))
            .collect(),
    };
    let row_count = reading.row_count();
    tx.execute(
        "INSERT INTO account_readings \
             (taken_at, purpose, session_id, balance_pec, is_keyframe, prev_reading_id, \
              row_count) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            reading.taken_at(),
            purpose.as_str(),
            session_id,
            reading.balance().0,
            i64::from(frame == Frame::Key),
            latest.as_ref().map(|latest| latest.id.0),
            i64::try_from(row_count).map_err(|_| AccountError::Stored("too many rows"))?,
        ],
    )?;
    let id = ReadingId(tx.last_insert_rowid());
    {
        let mut insert = tx.prepare(
            "INSERT INTO account_reading_holdings \
                 (reading_id, item_name, container_kind, container_name, quantity, value_pec, \
                  row_count, rows_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for (key, holding) in &writes {
            let pairs: Vec<[i64; 2]> = holding
                .rows()
                .iter()
                .map(|row| [row.quantity, row.value.0])
                .collect();
            let rows_json = serde_json::to_string(&pairs)
                .map_err(|_| AccountError::Stored("a holding's rows do not serialise"))?;
            insert.execute(rusqlite::params![
                id.0,
                key.name,
                key.container.kind().as_str(),
                key.container.name(),
                holding.quantity(),
                holding.value().0,
                i64::try_from(holding.row_count())
                    .map_err(|_| AccountError::Stored("too many rows"))?,
                rows_json,
            ])?;
        }
    }
    tx.commit()?;
    Ok(ReadingHeader {
        id,
        taken_at: reading.taken_at(),
        purpose,
        session_id: session_id.map(str::to_string),
        balance: reading.balance(),
        frame,
        prev: latest.map(|latest| latest.id),
        row_count,
    })
}

/// Store a reading through the writer.
pub async fn record(
    db: &Db,
    reading: Reading,
    purpose: Purpose,
    session_id: Option<String>,
) -> Result<ReadingHeader, AccountError> {
    db.with_writer(move |conn| {
        Ok(record_reading(
            conn,
            &reading,
            purpose,
            session_id.as_deref(),
        ))
    })
    .await?
}

/// The readings a session's measure may use, rebuilt whole and in the
/// order they were stored: every reading taken inside the window and every
/// reading stored between those, with the reading stored just before them
/// and the one stored just after them (the measure decides whether those
/// are close enough to the window's ends to use).
pub fn readings_for_window(
    conn: &rusqlite::Connection,
    window: &SessionWindow,
) -> Result<Vec<StoredReading>, AccountError> {
    // id-order: insertion
    let (first_inside, last_inside): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT MIN(id), MAX(id) FROM account_readings \
         WHERE taken_at > ?1 AND (?2 IS NULL OR taken_at < ?2)",
        rusqlite::params![window.started_at, window.ended_at],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let neighbour = |sql: &str, id: i64| -> Result<Option<i64>, AccountError> {
        Ok(conn
            .query_row(sql, [id], |row| row.get::<_, i64>(0))
            .optional()?)
    };
    // id-order: insertion
    const STORED_BEFORE: &str =
        "SELECT id FROM account_readings WHERE id < ?1 ORDER BY id DESC LIMIT 1";
    // id-order: insertion
    const STORED_AFTER: &str = "SELECT id FROM account_readings WHERE id > ?1 ORDER BY id LIMIT 1";
    let (from, to) = match (first_inside, last_inside) {
        (Some(first), Some(last)) => (
            neighbour(STORED_BEFORE, first)?.unwrap_or(first),
            neighbour(STORED_AFTER, last)?.unwrap_or(last),
        ),
        _ => {
            // Nothing inside: the last reading stored at or before the
            // start, and the one stored after it.
            let opening: Option<i64> = conn
                .query_row(
                    // id-order: insertion
                    "SELECT id FROM account_readings WHERE taken_at <= ?1 \
                     ORDER BY id DESC LIMIT 1",
                    [window.started_at],
                    |row| row.get(0),
                )
                .optional()?;
            match opening {
                Some(opening) => (
                    opening,
                    neighbour(STORED_AFTER, opening)?.unwrap_or(opening),
                ),
                None => match neighbour(STORED_AFTER, 0)? {
                    Some(first) => (first, first),
                    None => return Ok(Vec::new()),
                },
            }
        }
    };
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            // id-order: insertion
            "SELECT id FROM account_readings WHERE id BETWEEN ?1 AND ?2 ORDER BY id",
        )?;
        let ids = stmt
            .query_map([from, to], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids
    };

    load_in_order(conn, &ids)
}

/// A session's readings, rebuilt whole, in the order they were stored.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionReadings {
    pub readings: Vec<StoredReading>,
    /// The session's own readings among them, when any reading is tagged
    /// with it. `None` when none is, and the readings were chosen by its
    /// window instead.
    pub own: Option<std::ops::Range<usize>>,
}

/// The readings a session's measure uses: the ones tagged with it, in the
/// order they were stored, with the reading stored just before the first
/// of them and the one stored just after the last (the measure uses those
/// only to read an end its own readings leave unread). Readings tagged
/// with another session are never taken among its own. When no reading is
/// tagged with it (readings recorded without a session), they are chosen
/// by its window as [`readings_for_window`] does.
pub fn readings_for_session(
    conn: &rusqlite::Connection,
    session_id: &str,
    window: &SessionWindow,
) -> Result<SessionReadings, AccountError> {
    let tagged: Vec<i64> = {
        let mut stmt = conn.prepare(
            // id-order: insertion
            "SELECT id FROM account_readings WHERE session_id = ?1 ORDER BY id",
        )?;
        let ids = stmt
            .query_map([session_id], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids
    };
    let (Some(&first), Some(&last)) = (tagged.first(), tagged.last()) else {
        return Ok(SessionReadings {
            readings: readings_for_window(conn, window)?,
            own: None,
        });
    };
    let neighbour = |sql: &str, id: i64| -> Result<Option<i64>, AccountError> {
        Ok(conn
            .query_row(sql, [id], |row| row.get::<_, i64>(0))
            .optional()?)
    };
    // id-order: insertion
    const STORED_BEFORE: &str =
        "SELECT id FROM account_readings WHERE id < ?1 ORDER BY id DESC LIMIT 1";
    // id-order: insertion
    const STORED_AFTER: &str = "SELECT id FROM account_readings WHERE id > ?1 ORDER BY id LIMIT 1";
    let before = neighbour(STORED_BEFORE, first)?;
    let after = neighbour(STORED_AFTER, last)?;
    let mut ids: Vec<i64> = Vec::with_capacity(tagged.len() + 2);
    ids.extend(before);
    ids.extend(&tagged);
    ids.extend(after);
    let start = usize::from(before.is_some());
    Ok(SessionReadings {
        readings: load_in_order(conn, &ids)?,
        own: Some(start..start + tagged.len()),
    })
}

/// These stored readings, rebuilt whole, in the order given.
fn load_in_order(
    conn: &rusqlite::Connection,
    ids: &[i64],
) -> Result<Vec<StoredReading>, AccountError> {
    let mut readings: Vec<StoredReading> = Vec::with_capacity(ids.len());
    let mut state: BTreeMap<HoldingKey, Holding> = BTreeMap::new();
    for &id in ids {
        let header =
            header(conn, ReadingId(id))?.ok_or(AccountError::Stored("a listed reading is gone"))?;
        let follows_the_last = readings
            .last()
            .is_some_and(|last| header.prev == Some(last.header.id));
        if header.frame == Frame::Delta && follows_the_last {
            apply(&mut state, stored_holdings(conn, header.id)?);
        } else {
            state = state_at(conn, &header)?.0;
        }
        readings.push(rebuild(header, state.clone())?);
    }
    Ok(readings)
}

/// A session's cached measure, as it stands against the current rules.
#[derive(Debug, Clone, PartialEq)]
pub enum CachedMeasure {
    /// Computed by the current rules.
    Current {
        measure: Box<SessionMeasure>,
        sealed_at: Option<f64>,
    },
    /// Computed by other rules, or no longer decodable: recompute it.
    Stale {
        sealed_at: Option<f64>,
    },
    Missing,
}

/// The cached measure of one session.
pub fn cached_measure(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<CachedMeasure, AccountError> {
    let row = conn
        .query_row(
            "SELECT measure_version, measure_json, sealed_at \
             FROM account_session_measures WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((version, json, sealed_at)) = row else {
        return Ok(CachedMeasure::Missing);
    };
    // Older rules, or a later build's: either way not these rules.
    if version != MEASURE_VERSION {
        return Ok(CachedMeasure::Stale { sealed_at });
    }
    Ok(match serde_json::from_str::<SessionMeasure>(&json) {
        Ok(measure) => CachedMeasure::Current {
            measure: Box::new(measure),
            sealed_at,
        },
        Err(_) => CachedMeasure::Stale { sealed_at },
    })
}

/// A session's measure computed from its stored readings, with the
/// readings it used, ready to cache.
#[derive(Debug, Clone, PartialEq)]
pub struct ComputedMeasure {
    pub measure: SessionMeasure,
    first_reading: Option<ReadingId>,
    last_reading: Option<ReadingId>,
    /// The session's tagged readings when it was computed: how many, and
    /// the last stored. A cache write checks they still hold.
    tagged: TaggedReadings,
}

/// How many readings are tagged with a session, and the last of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaggedReadings {
    count: i64,
    last: Option<i64>,
}

fn tagged_readings(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<TaggedReadings, AccountError> {
    Ok(conn.query_row(
        // id-order: insertion (the last reading stored, not the latest taken)
        "SELECT COUNT(*), MAX(id) FROM account_readings WHERE session_id = ?1",
        [session_id],
        |row| {
            Ok(TaggedReadings {
                count: row.get(0)?,
                last: row.get(1)?,
            })
        },
    )?)
}

/// Compute a session's measure from its stored readings. A read: run it
/// on a reader.
pub fn compute_session_measure(
    conn: &rusqlite::Connection,
    session_id: &str,
    window: &SessionWindow,
    roles: &dyn RoleSource,
) -> Result<ComputedMeasure, AccountError> {
    let SessionReadings {
        readings: stored,
        own,
    } = readings_for_session(conn, session_id, window)?;
    let readings: Vec<Reading> = stored.iter().map(|stored| stored.reading.clone()).collect();
    // The session's own baseline: the reading it started.
    let baseline = stored.iter().position(|stored| {
        stored.header.purpose == Purpose::Baseline
            && stored.header.session_id.as_deref() == Some(session_id)
    });
    let outcome = match own {
        Some(own) => measure_own_readings(window, &readings, own, baseline, roles)?,
        None => measure_session_with_baseline(window, &readings, baseline, roles)?,
    };
    let used = &stored[outcome.used.clone()];
    Ok(ComputedMeasure {
        measure: outcome.measure,
        first_reading: used.first().map(|stored| stored.header.id),
        last_reading: used.last().map(|stored| stored.header.id),
        tagged: tagged_readings(conn, session_id)?,
    })
}

/// Cache a computed measure only while it is still the truth, in one
/// short transaction: the session still exists, its tagged readings are
/// the ones the computation saw, and it would not replace a sealed measure
/// computed from more readings. `Ok(false)` when it is not written; that is
/// no error, and the next read computes again.
pub fn cache_session_measure_if_current(
    conn: &mut rusqlite::Connection,
    session_id: &str,
    computed: &ComputedMeasure,
    sealed_at: Option<f64>,
    computed_at: f64,
) -> Result<bool, AccountError> {
    let tx = conn.transaction()?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM tracking_sessions WHERE id = ?1)",
        [session_id],
        |row| row.get(0),
    )?;
    if !exists || tagged_readings(&tx, session_id)? != computed.tagged {
        return Ok(false);
    }
    let sealed_from: Option<i64> = tx
        .query_row(
            "SELECT reading_count FROM account_session_measures \
             WHERE session_id = ?1 AND sealed_at IS NOT NULL",
            [session_id],
            |row| row.get(0),
        )
        .optional()?;
    let reading_count = i64::try_from(computed.measure.coverage.reading_count)
        .map_err(|_| AccountError::Stored("too many readings"))?;
    if sealed_from.is_some_and(|sealed| sealed > reading_count) {
        return Ok(false);
    }
    cache_session_measure(&tx, session_id, computed, sealed_at, computed_at)?;
    tx.commit()?;
    Ok(true)
}

/// Cache a computed measure. `sealed_at` marks it final: the session has
/// ended and its closing readings are in. A short write.
pub fn cache_session_measure(
    conn: &rusqlite::Connection,
    session_id: &str,
    computed: &ComputedMeasure,
    sealed_at: Option<f64>,
    computed_at: f64,
) -> Result<(), AccountError> {
    let measure = &computed.measure;
    let coverage = measure.coverage;
    let json = serde_json::to_string(measure)
        .map_err(|_| AccountError::Stored("a measure does not serialise"))?;
    let count = |n: usize| i64::try_from(n).map_err(|_| AccountError::Stored("too many readings"));
    conn.execute(
        "INSERT INTO account_session_measures \
             (session_id, first_reading_id, last_reading_id, sealed_at, coverage, \
              reading_count, gap_count, measure_version, measure_json, computed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT(session_id) DO UPDATE SET \
             first_reading_id = excluded.first_reading_id, \
             last_reading_id = excluded.last_reading_id, \
             sealed_at = excluded.sealed_at, \
             coverage = excluded.coverage, \
             reading_count = excluded.reading_count, \
             gap_count = excluded.gap_count, \
             measure_version = excluded.measure_version, \
             measure_json = excluded.measure_json, \
             computed_at = excluded.computed_at",
        rusqlite::params![
            session_id,
            computed.first_reading.map(|id| id.0),
            computed.last_reading.map(|id| id.0),
            sealed_at,
            coverage.level.as_str(),
            count(coverage.reading_count)?,
            count(coverage.gap_count)?,
            MEASURE_VERSION,
            json,
            computed_at,
        ],
    )?;
    Ok(())
}

/// Compute a session's measure from its stored readings and cache it, on
/// one connection.
pub fn refresh_session_measure(
    conn: &rusqlite::Connection,
    session_id: &str,
    window: &SessionWindow,
    roles: &dyn RoleSource,
    sealed_at: Option<f64>,
    computed_at: f64,
) -> Result<SessionMeasure, AccountError> {
    let computed = compute_session_measure(conn, session_id, window, roles)?;
    cache_session_measure(conn, session_id, &computed, sealed_at, computed_at)?;
    Ok(computed.measure)
}

/// A session's measure: the cached one when the current rules sealed it,
/// otherwise recomputed from its readings and cached, keeping any seal.
pub fn session_measure(
    conn: &rusqlite::Connection,
    session_id: &str,
    window: &SessionWindow,
    roles: &dyn RoleSource,
    computed_at: f64,
) -> Result<SessionMeasure, AccountError> {
    let sealed_at = match cached_measure(conn, session_id)? {
        CachedMeasure::Current {
            measure,
            sealed_at: Some(_),
        } => return Ok(*measure),
        CachedMeasure::Current { sealed_at, .. } | CachedMeasure::Stale { sealed_at } => sealed_at,
        CachedMeasure::Missing => None,
    };
    refresh_session_measure(conn, session_id, window, roles, sealed_at, computed_at)
}
