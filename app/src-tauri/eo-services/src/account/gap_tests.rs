//! A session whose readings stop before it ends, billed across the gap
//! only when the chat log shows the gap clean.

use std::io::Write;

use super::amount::MICRO_PER_PEC;
use super::costs::{
    session_costs_with, sessions_with_stale_costs, Basis, CostBucket, GapBound, SessionCosts,
    Status, Unmeasured,
};
use super::reading::Container;
use super::roles::KnownRoles;
use super::store::{record_reading, Purpose, ReadingHeader};
use super::test_support::reading;
use crate::chatlog_span::ChatLogSource;
use crate::chatlog_time::ChatLogClock;
use crate::db::migrated_in_memory;
use crate::session_costs::{self, AcrossGap, TrackedReason};

const C: Container = Container::Carried;
const PED: i64 = 1_000_000;
const PEC: i64 = MICRO_PER_PEC;

/// The session: started at 10 000 and stopped at 11 000. Its readings stop
/// at 10 180, when the sign-in drops; the next reading is taken at 12 800,
/// when the player signs in again, 30 minutes after the stop.
const STARTED: f64 = 10_000.0;
const STOPPED: f64 = 11_000.0;
const SIGNED_IN_AGAIN: f64 = 12_800.0;

/// What the account holds at one reading.
#[derive(Clone, Copy)]
struct State {
    ammo: i64,
    opalo: i64,
    fap: i64,
    balance: i64,
    /// Animal Oil Residue carried, before it is recycled.
    oil: i64,
    /// Nanocubes the recycling gave.
    nanocubes: i64,
    /// Whether the ingot has been moved to storage.
    ingot_stored: bool,
    chip: i64,
    repair_unit: i64,
}

const START: State = State {
    ammo: 1_000_000,
    opalo: 5_000,
    fap: 3_000,
    balance: 10_000,
    oil: 5_000,
    nanocubes: 0,
    ingot_stored: false,
    chip: 8_665,
    repair_unit: 500,
};

fn at(time: f64, state: State) -> super::Reading {
    let mut rows: Vec<(&str, Container, i64, i64)> = vec![
        ("Opalo", C, 1, state.opalo),
        ("Universal Ammo", C, state.ammo, state.ammo / 100),
        ("Odd FAP", C, 1, state.fap),
        ("Teleportation Chip 1", C, 1, state.chip),
        ("Portable Repair Unit (L)", C, 1, state.repair_unit),
    ];
    if state.oil > 0 {
        rows.push(("Animal Oil Residue", C, state.oil, state.oil / 100));
    }
    if state.nanocubes > 0 {
        rows.push(("Nanocube", C, state.nanocubes, state.nanocubes));
    }
    let ingot = if state.ingot_stored {
        Container::Storage {
            planet: "Calypso".to_string(),
        }
    } else {
        C
    };
    rows.push(("Lysterium Ingot", ingot, 10, 50));
    reading(time, state.balance, &rows)
}

/// The play the readings saw: 10 000 ammo and 10 PEC of the weapon a
/// minute, the first aid pack 2 PEC a minute.
fn played(state: State, minutes: i64) -> State {
    State {
        ammo: state.ammo - 10_000 * minutes,
        opalo: state.opalo - 10 * minutes,
        fap: state.fap - 2 * minutes,
        ..state
    }
}

/// The player's weapon and first aid pack, and the session, whose tracked
/// play shot and healed.
fn the_session(conn: &rusqlite::Connection) {
    conn.execute(
        "INSERT INTO equipment_library (name, item_type, properties_json) VALUES \
         ('Opalo', 'weapon', '{\"weapon_entity\": {\"name\": \"Opalo\"}}'), \
         ('Odd FAP', 'healing', '{}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, dangling_cost, \
           heal_cost) VALUES ('drop', ?1, ?2, 0, 1.0, 0.2)",
        rusqlite::params![STARTED, STOPPED],
    )
    .unwrap();
}

/// The readings up to the drop: one just before the start, and three of
/// the session's own, a minute apart. Returns the state at the drop.
fn read_until_the_drop(
    conn: &mut rusqlite::Connection,
    extra: impl Fn(State, usize) -> State,
) -> State {
    record_reading(conn, &at(9_990.0, START), Purpose::Session, None).unwrap();
    let mut state = START;
    for k in 1..=3 {
        state = extra(played(state, 1), k);
        let time = STARTED + 60.0 * k as f64;
        record_reading(conn, &at(time, state), Purpose::Session, Some("drop")).unwrap();
    }
    state
}

/// The reading taken on signing in again: three more minutes of the
/// session's play since the drop, and in the half hour after it the oil
/// recycled into Nanocubes and the ingot moved to storage.
fn signed_in_again(conn: &mut rusqlite::Connection, dropped: State) -> ReadingHeader {
    let after = State {
        oil: 0,
        nanocubes: 50,
        ingot_stored: true,
        ..played(dropped, 3)
    };
    signed_in_with(conn, after)
}

fn signed_in_with(conn: &mut rusqlite::Connection, state: State) -> ReadingHeader {
    record_reading(conn, &at(SIGNED_IN_AGAIN, state), Purpose::Connect, None).unwrap()
}

fn stamp(at: f64) -> String {
    chrono::DateTime::from_timestamp(at as i64, 0)
        .unwrap()
        .naive_utc()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// A chat log from `from` to 13 500: a chat line every ten seconds, the
/// session's shots and loot inside it, and `extra` lines.
fn chat_log(from: f64, extra: &[(f64, &str)]) -> tempfile::NamedTempFile {
    chat_log_until(from, 13_500.0, extra)
}

/// A chat line every ten seconds over `from..=until`, the session's shots
/// and loot inside it, and `extra` lines.
fn chat_lines(from: f64, until: f64, extra: &[(f64, &str)]) -> Vec<(f64, String)> {
    let mut lines: Vec<(f64, String)> = Vec::new();
    let mut time = from;
    while time <= until {
        lines.push((
            time,
            "[#calypso] [Someone Else] anyone selling oil?".to_string(),
        ));
        time += 10.0;
    }
    for shot in [10_100.0, 10_500.0, 10_900.0] {
        if shot >= from && shot <= until {
            lines.push((
                shot,
                "[System] [] You inflicted 12.5 points of damage".to_string(),
            ));
            lines.push((
                shot + 2.0,
                "[System] [] You received Animal Oil Residue x (50) Value: 0.50 PED".to_string(),
            ));
        }
    }
    for (time, line) in extra {
        lines.push((*time, (*line).to_string()));
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    lines
}

/// [`chat_log`], stopping at `until`.
fn chat_log_until(from: f64, until: f64, extra: &[(f64, &str)]) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    write_lines(&file, chat_lines(from, until, extra));
    file
}

/// Append lines to a log, as the game writes on.
fn write_lines(file: &tempfile::NamedTempFile, lines: Vec<(f64, String)>) {
    let mut out = file.as_file();
    for (time, line) in lines {
        writeln!(out, "{} {line}", stamp(time)).unwrap();
    }
    out.flush().unwrap();
}

fn source(file: &tempfile::NamedTempFile) -> ChatLogSource {
    ChatLogSource::at(file.path(), ChatLogClock::pinned(0))
}

fn costs_with(conn: &mut rusqlite::Connection, chat_log: Option<&ChatLogSource>) -> SessionCosts {
    session_costs_with(conn, "drop", None, chat_log, 20_000.0, None)
        .unwrap()
        .expect("a known session")
}

fn basis(costs: &SessionCosts, bucket: CostBucket) -> Basis {
    costs.bucket(bucket).expect("every bucket").basis
}

fn measured(costs: &SessionCosts, bucket: CostBucket) -> i64 {
    match basis(costs, bucket) {
        Basis::Measured(amount) => amount.micro_ped(),
        other => panic!("{bucket:?} not measured whole: {other:?}"),
    }
}

fn kept_verdict(conn: &rusqlite::Connection) -> Option<String> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT verdict FROM account_session_gap_verdicts WHERE session_id = 'drop'",
        [],
        |row| row.get(0),
    )
    .optional()
    .unwrap()
}

fn stored_header(conn: &rusqlite::Connection) -> Option<(String, Option<String>)> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT status, reason FROM account_session_costs WHERE session_id = 'drop'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .unwrap()
}

/// Every bucket keeps its tracked figure for `reason`, and the stored
/// projection and the resolution say so.
fn kept_tracked(
    conn: &rusqlite::Connection,
    costs: &SessionCosts,
    reason: Unmeasured,
    tracked: TrackedReason,
) {
    assert_eq!(costs.status, Status::Unmeasured(reason));
    for cost in &costs.buckets {
        let expected = if cost.bucket == CostBucket::Enhancer {
            Unmeasured::TrackedOnly
        } else {
            reason
        };
        assert_eq!(cost.basis, Basis::Unmeasured(expected), "{:?}", cost.bucket);
    }
    assert_eq!(
        stored_header(conn),
        Some(("unmeasured".to_string(), Some(reason.as_str().to_string())))
    );
    let resolved = session_costs::resolve(conn, "drop").unwrap();
    assert_eq!(
        resolved.basis(session_costs::Bucket::Weapon),
        session_costs::Basis::Tracked(tracked)
    );
    assert_eq!(resolved.across_gap, None);
}

/// The sign-in drops mid-session, and in the half hour between the stop
/// and signing in again the player only recycled (the chat log says what
/// the recycling gave, which is no play) and moved items to storage. The
/// readings either side of the gap hold the session's whole change, and
/// the session is billed from them.
#[test]
fn a_session_whose_sign_in_dropped_is_billed_across_a_clean_gap() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    let log = chat_log(
        9_000.0,
        &[(
            11_600.0,
            "[System] [] You received Nanocube x (50) Value: 0.50 PED",
        )],
    );
    let chat_log = source(&log);

    // Until a reading comes after the gap, nothing is weighed, and nothing
    // is stored: the session stays to be computed again.
    let waiting = costs_with(&mut conn, Some(&chat_log));
    assert_eq!(
        waiting.status,
        Status::Unmeasured(Unmeasured::AwaitingGapReading)
    );
    assert_eq!(stored_header(&conn), None);
    let roles = KnownRoles::read(&conn, None).unwrap();
    assert_eq!(
        sessions_with_stale_costs(&conn, roles.fingerprint()).unwrap(),
        ["drop"]
    );

    let after = signed_in_again(&mut conn, dropped);
    let costs = costs_with(&mut conn, Some(&chat_log));
    let Status::AcrossGap(bounds) = costs.status else {
        panic!("measured across the gap: {:?}", costs.status);
    };
    assert_eq!(bounds.opening, None);
    assert_eq!(
        bounds.closing,
        Some(GapBound {
            reading_id: after.id.0,
            outside_seconds: 1_800,
            gap_seconds: 2_620,
        })
    );
    // Six minutes of play: 60 000 ammo and 60 PEC of the weapon, 12 PEC of
    // the first aid pack. The recycling and the move cost nothing.
    assert_eq!(measured(&costs, CostBucket::Weapon), 6 * PED + 60 * PEC);
    assert_eq!(measured(&costs, CostBucket::Healing), 12 * PEC);
    for bucket in [
        CostBucket::Armour,
        CostBucket::Consumables,
        CostBucket::Unclassified,
    ] {
        assert_eq!(measured(&costs, bucket), 0, "{bucket:?}");
    }

    // Stored as its own kind, with the readings that bound it, and the chat
    // log's verdict over the span kept apart.
    assert_eq!(stored_header(&conn), Some(("acrossGap".to_string(), None)));
    assert_eq!(kept_verdict(&conn), Some("clean".to_string()));
    let (first, last, closing, outside, gap): (i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT c.first_reading_id, c.last_reading_id, g.closing_reading_id, \
                    g.closing_outside_seconds, g.closing_gap_seconds \
             FROM account_session_costs c JOIN account_session_cost_gaps g USING (session_id) \
             WHERE c.session_id = 'drop'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        (last, closing, outside, gap),
        (after.id.0, after.id.0, 1_800, 2_620)
    );
    assert_eq!(first, after.id.0 - 4);
    // Read back whole, it is the same.
    assert_eq!(costs_with(&mut conn, None), costs);

    // The session's figures follow, and say how long the gap was.
    let resolved = session_costs::resolve(&conn, "drop").unwrap();
    assert_eq!(
        resolved.basis(session_costs::Bucket::Weapon),
        session_costs::Basis::Measured(6.6)
    );
    assert_eq!(resolved.across_gap, Some(AcrossGap { gap_seconds: 2_620 }));
    let detail = crate::tracking_reads::get_session_read(&conn, "drop", 20_000.0)
        .unwrap()
        .expect("the session");
    assert_eq!(
        detail["summary"]["costBreakdown"]["measuredAcrossGapMinutes"],
        serde_json::json!(44)
    );
}

#[test]
fn combat_outside_the_session_in_the_gap_keeps_tracked_figures() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    signed_in_again(&mut conn, dropped);
    // A shot twenty minutes after the stop: play the session did not track
    // is in the gap's change.
    let log = chat_log(
        9_000.0,
        &[(12_200.0, "[System] [] You inflicted 3.0 points of damage")],
    );
    let costs = costs_with(&mut conn, Some(&source(&log)));
    kept_tracked(
        &conn,
        &costs,
        Unmeasured::PlayInGap,
        TrackedReason::PlayInGap,
    );
}

#[test]
fn a_chat_log_starting_after_the_gap_begins_keeps_tracked_figures() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    signed_in_again(&mut conn, dropped);
    let log = chat_log(11_500.0, &[]);
    let costs = costs_with(&mut conn, Some(&source(&log)));
    kept_tracked(
        &conn,
        &costs,
        Unmeasured::ChatLogMissesGap,
        TrackedReason::ChatLogMissesGap,
    );
}

#[test]
fn without_a_chat_log_the_gap_is_not_billed() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    signed_in_again(&mut conn, dropped);
    let costs = costs_with(&mut conn, None);
    kept_tracked(
        &conn,
        &costs,
        Unmeasured::ChatLogUnchecked,
        TrackedReason::ChatLogUnchecked,
    );
}

/// Until the live tail has placed the log's stamps in time, the gap is not
/// weighed and nothing is stored; once it has, the gap is billed.
#[test]
fn a_gap_is_weighed_once_the_chat_log_s_time_base_is_known() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    signed_in_again(&mut conn, dropped);
    let log = chat_log(9_000.0, &[]);
    let unsettled = ChatLogSource::at(log.path(), ChatLogClock::observed());
    let costs = costs_with(&mut conn, Some(&unsettled));
    assert_eq!(
        costs.status,
        Status::Unmeasured(Unmeasured::GapCheckPending)
    );
    assert_eq!(stored_header(&conn), None);
    let costs = costs_with(&mut conn, Some(&source(&log)));
    assert!(
        matches!(costs.status, Status::AcrossGap(_)),
        "{:?}",
        costs.status
    );
}

/// A teleport in the gap may be the player's after the session: travel is
/// not billed, while the rest is, the field logistics worn inside the
/// session included.
#[test]
fn overhead_in_the_gap_step_is_not_billed() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let dropped = read_until_the_drop(&mut conn, |state, k| State {
        repair_unit: if k >= 2 { 490 } else { 500 },
        ..state
    });
    signed_in_with(
        &mut conn,
        State {
            chip: 8_663,
            ..played(dropped, 3)
        },
    );
    let log = chat_log(9_000.0, &[]);
    let costs = costs_with(&mut conn, Some(&source(&log)));
    assert!(
        matches!(costs.status, Status::AcrossGap(_)),
        "{:?}",
        costs.status
    );
    assert_eq!(
        basis(&costs, CostBucket::Travel),
        Basis::Unmeasured(Unmeasured::OverheadInGap)
    );
    assert_eq!(measured(&costs, CostBucket::FieldLogistics), 10 * PEC);
    assert_eq!(measured(&costs, CostBucket::Weapon), 6 * PED + 60 * PEC);
    let travel: (String, Option<String>) = conn
        .query_row(
            "SELECT basis, reason FROM account_session_cost_buckets \
             WHERE session_id = 'drop' AND bucket = 'travel'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        travel,
        ("unmeasured".to_string(), Some("overheadInGap".to_string()))
    );
}

/// A repair in the gap restores decay the step would otherwise show: the
/// gap's change no longer holds the session's whole wear.
#[test]
fn a_repair_in_the_gap_keeps_tracked_figures() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    let played = played(dropped, 3);
    signed_in_with(
        &mut conn,
        State {
            fap: 3_000,
            balance: played.balance - (3_000 - played.fap),
            ..played
        },
    );
    let log = chat_log(9_000.0, &[]);
    let costs = costs_with(&mut conn, Some(&source(&log)));
    kept_tracked(
        &conn,
        &costs,
        Unmeasured::GapUnaccounted,
        TrackedReason::GapUnaccounted,
    );
}

/// Healing wear in the gap, in a session whose own play never healed, is
/// not the session's kind of play: the gap is not billed.
#[test]
fn a_cost_the_session_s_play_never_had_keeps_tracked_figures() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    conn.execute("UPDATE tracking_sessions SET heal_cost = 0", [])
        .unwrap();
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    signed_in_again(&mut conn, dropped);
    let log = chat_log(9_000.0, &[]);
    let costs = costs_with(&mut conn, Some(&source(&log)));
    kept_tracked(
        &conn,
        &costs,
        Unmeasured::GapUnaccounted,
        TrackedReason::GapUnaccounted,
    );
}

/// A session started while signed out, read from part-way through, is
/// measured across the gap before its start.
#[test]
fn a_session_read_from_part_way_is_billed_across_the_gap_before_it() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    // The last reading before it, twenty minutes before the start.
    record_reading(&mut conn, &at(8_800.0, START), Purpose::Connect, None).unwrap();
    let mut state = START;
    for time in [10_400.0, 10_460.0, 10_520.0, 11_030.0] {
        state = played(state, 1);
        record_reading(&mut conn, &at(time, state), Purpose::Session, Some("drop")).unwrap();
    }
    // The last reading is 510 seconds after the one before: inside the
    // session, a gap of its own, so the readings stop on the way.
    let log = chat_log(8_000.0, &[]);
    let costs = costs_with(&mut conn, Some(&source(&log)));
    assert_eq!(
        costs.status,
        Status::Unmeasured(Unmeasured::PartialCoverage),
        "a gap inside the session as well is not billed"
    );

    // Read again, the minute cadence held to the end.
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let before = record_reading(&mut conn, &at(8_800.0, START), Purpose::Connect, None).unwrap();
    let mut state = START;
    for k in 0..11 {
        state = played(state, 1);
        let time = 10_400.0 + 60.0 * k as f64;
        record_reading(&mut conn, &at(time, state), Purpose::Session, Some("drop")).unwrap();
    }
    let costs = costs_with(&mut conn, Some(&source(&log)));
    let Status::AcrossGap(bounds) = costs.status else {
        panic!("measured across the gap: {:?}", costs.status);
    };
    assert_eq!(bounds.closing, None);
    assert_eq!(
        bounds.opening,
        Some(GapBound {
            reading_id: before.id.0,
            outside_seconds: 1_200,
            gap_seconds: 1_600,
        })
    );
    assert_eq!(measured(&costs, CostBucket::Weapon), 11 * PED + 110 * PEC);
}

/// A session its own readings measure whole is measured as before,
/// whatever the chat log holds outside it.
#[test]
fn a_complete_measure_is_unaffected_by_the_chat_log() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    let mut state = dropped;
    for k in 4..=17 {
        state = played(state, 1);
        let time = STARTED + 60.0 * k as f64;
        record_reading(&mut conn, &at(time, state), Purpose::Session, Some("drop")).unwrap();
    }
    let log = chat_log(
        9_000.0,
        &[(12_200.0, "[System] [] You inflicted 3.0 points of damage")],
    );
    let costs = costs_with(&mut conn, Some(&source(&log)));
    assert_eq!(costs.status, Status::Measured);
    assert_eq!(measured(&costs, CostBucket::Weapon), 17 * PED + 170 * PEC);
    assert_eq!(stored_header(&conn), Some(("measured".to_string(), None)));
    let gaps: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM account_session_cost_gaps",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(gaps, 0);
    assert_eq!(
        session_costs::resolve(&conn, "drop").unwrap().across_gap,
        None
    );
}

/// A session row whose tracked play shot and healed, as `the_session`'s.
fn another_session(conn: &rusqlite::Connection, id: &str, started: f64, ended: f64) {
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, dangling_cost, \
           heal_cost) VALUES (?1, ?2, ?3, 0, 1.0, 0.2)",
        rusqlite::params![id, started, ended],
    )
    .unwrap();
}

fn costs_of_with(
    conn: &mut rusqlite::Connection,
    id: &str,
    chat_log: Option<&ChatLogSource>,
) -> SessionCosts {
    session_costs_with(conn, id, None, chat_log, 20_000.0, None)
        .unwrap()
        .expect("a known session")
}

/// A log the game has not written past the gap yet has not missed it: the
/// gap is weighed once the game writes on, and nothing is kept before.
#[test]
fn a_chat_log_not_yet_written_past_the_gap_is_weighed_once_it_is() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    signed_in_again(&mut conn, dropped);
    // The game was closed at 12 000, before the reading after the gap.
    let log = chat_log_until(9_000.0, 12_000.0, &[]);
    let costs = costs_with(&mut conn, Some(&source(&log)));
    assert_eq!(
        costs.status,
        Status::Unmeasured(Unmeasured::ChatLogNotPastGap)
    );
    assert_eq!(stored_header(&conn), None);
    assert_eq!(kept_verdict(&conn), None);
    // The game is played again, and writes past the gap.
    write_lines(&log, chat_lines(12_010.0, 13_500.0, &[]));
    let costs = costs_with(&mut conn, Some(&source(&log)));
    assert!(
        matches!(costs.status, Status::AcrossGap(_)),
        "{:?}",
        costs.status
    );
}

/// The chat log's verdict over a span is taken once: a rebuild of the
/// projection over the same span (an item placed by a new library entry,
/// here) takes the kept verdict and never opens the log again.
#[test]
fn a_rebuild_over_the_same_span_takes_the_kept_verdict() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    signed_in_again(&mut conn, dropped);
    let log = chat_log(9_000.0, &[]);
    let first = costs_with(&mut conn, Some(&source(&log)));
    assert!(matches!(first.status, Status::AcrossGap(_)));

    conn.execute(
        "INSERT INTO equipment_library (name, item_type, properties_json) \
         VALUES ('Spare FAP', 'healing', '{}')",
        [],
    )
    .unwrap();
    let roles = KnownRoles::read(&conn, None).unwrap();
    assert_eq!(
        sessions_with_stale_costs(&conn, roles.fingerprint()).unwrap(),
        ["drop"]
    );
    let never_read = ChatLogSource::new(
        || panic!("the chat log was read again over a span it has a verdict for"),
        ChatLogClock::pinned(0),
    );
    let rebuilt = costs_with(&mut conn, Some(&never_read));
    assert_eq!(rebuilt.status, first.status);
    assert_eq!(rebuilt.roles_fingerprint, roles.fingerprint());
    assert!(sessions_with_stale_costs(&conn, roles.fingerprint())
        .unwrap()
        .is_empty());
    assert_eq!(kept_verdict(&conn), Some("clean".to_string()));
}

/// The reading the next session took as it started closes the gap of the
/// one before it: nothing of the next session's play is in it yet. Taken
/// later than that, it holds the next session's play, and does not.
#[test]
fn the_next_session_s_baseline_may_close_the_gap() {
    for (taken, closes) in [(12_801.0, true), (12_815.0, false)] {
        let mut conn = migrated_in_memory().unwrap();
        the_session(&conn);
        another_session(&conn, "next", SIGNED_IN_AGAIN, 13_400.0);
        let dropped = read_until_the_drop(&mut conn, |state, _| state);
        let baseline = record_reading(
            &mut conn,
            &at(taken, played(dropped, 3)),
            Purpose::Baseline,
            Some("next"),
        )
        .unwrap();
        let log = chat_log(9_000.0, &[]);
        let costs = costs_with(&mut conn, Some(&source(&log)));
        if closes {
            let Status::AcrossGap(bounds) = costs.status else {
                panic!("measured across the gap: {:?}", costs.status);
            };
            assert_eq!(
                bounds.closing,
                Some(GapBound {
                    reading_id: baseline.id.0,
                    outside_seconds: 1_801,
                    gap_seconds: 2_621,
                })
            );
            assert_eq!(measured(&costs, CostBucket::Weapon), 6 * PED + 60 * PEC);
        } else {
            assert_eq!(
                costs.status,
                Status::Unmeasured(Unmeasured::PartialCoverage),
                "a baseline taken {taken}"
            );
        }
    }
}

/// A session tracked inside the gap played in it: the gap is never billed
/// to the session before it.
#[test]
fn a_session_tracked_in_the_gap_keeps_it_unbilled() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    another_session(&conn, "between", 11_500.0, 12_000.0);
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    signed_in_again(&mut conn, dropped);
    let log = chat_log(9_000.0, &[]);
    let costs = costs_with(&mut conn, Some(&source(&log)));
    assert_eq!(
        costs.status,
        Status::Unmeasured(Unmeasured::PartialCoverage)
    );
    assert_eq!(kept_verdict(&conn), None);
}

/// Read neither at its start nor at its end, a session is measured across
/// both gaps, each shown clean.
#[test]
fn a_session_is_billed_across_a_gap_at_both_ends() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    let before = record_reading(&mut conn, &at(8_800.0, START), Purpose::Connect, None).unwrap();
    let mut state = START;
    for time in [10_400.0, 10_460.0, 10_520.0] {
        state = played(state, 1);
        record_reading(&mut conn, &at(time, state), Purpose::Session, Some("drop")).unwrap();
    }
    let after = signed_in_with(&mut conn, played(state, 3));
    let log = chat_log(8_000.0, &[]);
    let costs = costs_with(&mut conn, Some(&source(&log)));
    let Status::AcrossGap(bounds) = costs.status else {
        panic!("measured across the gap: {:?}", costs.status);
    };
    assert_eq!(
        bounds.opening,
        Some(GapBound {
            reading_id: before.id.0,
            outside_seconds: 1_200,
            gap_seconds: 1_600,
        })
    );
    assert_eq!(
        bounds.closing,
        Some(GapBound {
            reading_id: after.id.0,
            outside_seconds: 1_800,
            gap_seconds: 2_280,
        })
    );
    assert_eq!(bounds.gap_seconds(), 3_880);
    assert_eq!(measured(&costs, CostBucket::Weapon), 6 * PED + 60 * PEC);
    // Play in the gap before it refuses it as well.
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    record_reading(&mut conn, &at(8_800.0, START), Purpose::Connect, None).unwrap();
    let mut state = START;
    for time in [10_400.0, 10_460.0, 10_520.0] {
        state = played(state, 1);
        record_reading(&mut conn, &at(time, state), Purpose::Session, Some("drop")).unwrap();
    }
    signed_in_with(&mut conn, played(state, 3));
    let log = chat_log(
        8_000.0,
        &[(9_500.0, "[System] [] You took 4.0 points of damage")],
    );
    let costs = costs_with(&mut conn, Some(&source(&log)));
    assert_eq!(costs.status, Status::Unmeasured(Unmeasured::PlayInGap));
}

/// One reading closes the gap of the session before it and opens the gap
/// of the session after it: each is billed what changed on its own side,
/// and between them they bill the whole change once.
#[test]
fn two_sessions_sharing_a_bracketing_reading_never_share_its_change() {
    let mut conn = migrated_in_memory().unwrap();
    the_session(&conn);
    another_session(&conn, "later", 12_000.0, 12_500.0);
    let dropped = read_until_the_drop(&mut conn, |state, _| state);
    let shared = played(dropped, 3);
    let between = record_reading(&mut conn, &at(11_500.0, shared), Purpose::Connect, None).unwrap();
    let mut state = shared;
    for time in [12_400.0, 12_460.0, 12_520.0] {
        state = played(state, 1);
        record_reading(&mut conn, &at(time, state), Purpose::Session, Some("later")).unwrap();
    }
    let log = chat_log(9_000.0, &[]);
    let chat_log = source(&log);
    let first = costs_of_with(&mut conn, "drop", Some(&chat_log));
    let second = costs_of_with(&mut conn, "later", Some(&chat_log));
    let (Status::AcrossGap(first_bounds), Status::AcrossGap(second_bounds)) =
        (first.status, second.status)
    else {
        panic!("both across a gap: {:?}, {:?}", first.status, second.status);
    };
    assert_eq!(
        first_bounds.closing.map(|end| end.reading_id),
        Some(between.id.0)
    );
    assert_eq!(
        second_bounds.opening.map(|end| end.reading_id),
        Some(between.id.0)
    );
    assert_eq!(first_bounds.last_reading_id, second_bounds.first_reading_id);
    assert_eq!(measured(&first, CostBucket::Weapon), 6 * PED + 60 * PEC);
    assert_eq!(measured(&second, CostBucket::Weapon), 3 * PED + 30 * PEC);
    // Nine minutes of play from the first reading to the last, billed once.
    let whole = (START.ammo - state.ammo) / 100 * PEC + (START.opalo - state.opalo) * PEC;
    assert_eq!(
        measured(&first, CostBucket::Weapon) + measured(&second, CostBucket::Weapon),
        whole
    );
}

/// A memory of an earlier run's settled offset, held in the test.
struct EarlierRun(Option<crate::chatlog_time::RememberedOffset>);

impl crate::chatlog_time::OffsetMemory for EarlierRun {
    fn recall(&self) -> Option<crate::chatlog_time::RememberedOffset> {
        self.0
    }
    fn remember(&self, _: crate::chatlog_time::RememberedOffset, _: i64) {}
}

/// The app restarted after the gap: the live tail settles the offset again
/// at 20 000, well after the gap began. At the value an earlier run
/// remembered (held since 9 000), the gap is placed by it and billed; at
/// another value, or with nothing remembered, it cannot be shown to have
/// held, and the gap is not billed.
#[test]
fn a_gap_across_a_restart_is_billed_when_the_offset_settles_again_unchanged() {
    use crate::chatlog_time::{ChatLogReading, RememberedOffset};
    let settled_again = chrono::DateTime::from_timestamp(20_000, 0).unwrap();
    let cases = [(Some(0), true), (Some(3_600), false), (None, false)];
    for (remembered, billed) in cases {
        let mut conn = migrated_in_memory().unwrap();
        the_session(&conn);
        let dropped = read_until_the_drop(&mut conn, |state, _| state);
        signed_in_again(&mut conn, dropped);
        let log = chat_log(9_000.0, &[]);
        let memory = EarlierRun(remembered.map(|offset_seconds| RememberedOffset {
            offset_seconds,
            held_since: 9_000,
        }));
        let clock = ChatLogClock::observed_remembering(std::sync::Arc::new(memory));
        clock.observe(
            ChatLogReading::new(settled_again.naive_utc()),
            settled_again,
        );
        let costs = costs_with(&mut conn, Some(&ChatLogSource::at(log.path(), clock)));
        if billed {
            assert!(
                matches!(costs.status, Status::AcrossGap(_)),
                "{remembered:?}: {:?}",
                costs.status
            );
        } else {
            assert_eq!(
                costs.status,
                Status::Unmeasured(Unmeasured::ChatLogUnchecked),
                "{remembered:?}"
            );
        }
    }
}
