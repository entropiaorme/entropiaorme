//! What the account did on its own, as far as the player has seen it.
//!
//! The readings record some things with nobody looking (see
//! [`super::follow`]) and find others that wait for the player's decision
//! (see [`super::proposals`]). Each is new to the player until they look at
//! what the account did: one watermark (`account_activity_seen`, unix
//! seconds) says how far they have, and it only moves forward.
//!
//! What counts is an event, the way the player reads it, never a row: a
//! step's recycling, its Shrapnel conversion, its sales to the Trade
//! Terminal together, each listing outcome it settled, and each proposal
//! still open. An arrival offered in a session's minutes is no news: it is
//! stored as a proposal only once the player reads those minutes, so it is
//! never counted here.

use rusqlite::OptionalExtension;

use super::proposals::{self, ProposalKind};
use super::AccountError;

/// What is new to the player in what the account did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActivityMarks {
    /// How far the player has seen (unix seconds).
    pub seen_through: f64,
    /// The events recorded or found since, by the readings on their own.
    pub unseen: i64,
}

/// How far the player has seen. A database from before the watermark
/// existed has every row seeded by its migration; none at all reads as
/// nothing seen.
fn seen_through(conn: &rusqlite::Connection) -> Result<f64, AccountError> {
    Ok(conn
        .query_row(
            "SELECT seen_through FROM account_activity_seen WHERE id = 1",
            [],
            |row| row.get::<_, f64>(0),
        )
        .optional()?
        .unwrap_or(0.0))
}

/// What is new to the player: the events the readings recorded or found
/// after the watermark. A read.
pub fn marks(conn: &rusqlite::Connection) -> Result<ActivityMarks, AccountError> {
    let seen = seen_through(conn)?;
    // A step's sales to the Trade Terminal are one event, however many
    // items it sold; every other part of a step is one of its own.
    let records: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ( \
             SELECT DISTINCT reading_id, \
                    CASE WHEN part LIKE 'terminal_sale:%' THEN 'terminal_sale' ELSE part END \
             FROM stock_records_from_account WHERE created_at > ?1)",
        [seen],
        |row| row.get(0),
    )?;
    // A listing outcome counts while it stands, as the history shows it:
    // one the player has since taken back is not news to look for.
    let listings: i64 = conn.query_row(
        "SELECT COUNT(*) FROM listing_outcomes_from_account o \
         JOIN auction_listings l ON l.id = o.listing_id \
         WHERE o.created_at > ?1 AND l.undone_at IS NULL \
           AND l.status = CASE o.outcome WHEN 'sold' THEN 'sold' ELSE 'expired' END",
        [seen],
        |row| row.get(0),
    )?;
    let found = proposals::pending(conn, None)?
        .iter()
        .filter(|proposal| proposal.kind != ProposalKind::Arrival && proposal.detected_at > seen)
        .count();
    let found = i64::try_from(found).map_err(|_| AccountError::Overflow)?;
    Ok(ActivityMarks {
        seen_through: seen,
        unseen: records + listings + found,
    })
}

/// The player has seen what the account did up to `through` (unix
/// seconds): the watermark moves there, never back and never past `now`.
/// Whether it moved.
pub fn mark_seen(
    conn: &rusqlite::Connection,
    through: f64,
    now: f64,
) -> Result<bool, AccountError> {
    if !through.is_finite() || !now.is_finite() {
        return Ok(false);
    }
    let through = through.min(now);
    let moved = conn.execute(
        "INSERT INTO account_activity_seen (id, seen_through, updated_at) VALUES (1, ?1, ?2) \
         ON CONFLICT (id) DO UPDATE SET seen_through = excluded.seen_through, \
                                        updated_at = excluded.updated_at \
         WHERE excluded.seen_through > account_activity_seen.seen_through",
        rusqlite::params![through, now],
    )?;
    Ok(moved > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrated_in_memory;

    fn db() -> rusqlite::Connection {
        migrated_in_memory().expect("a migrated database")
    }

    fn set_seen(conn: &rusqlite::Connection, at: f64) {
        conn.execute(
            "UPDATE account_activity_seen SET seen_through = ?1 WHERE id = 1",
            [at],
        )
        .unwrap();
    }

    fn record(conn: &rusqlite::Connection, reading: i64, part: &str, record: &str, at: f64) {
        conn.execute(
            "INSERT INTO stock_records_from_account \
                 (record_kind, record_id, reading_id, prev_reading_id, seen_from, seen_to, \
                  part, item_name, created_at) \
             VALUES ('trade', ?1, ?2, ?3, 0, 1, ?4, 'Item', ?5)",
            rusqlite::params![record, reading, reading - 1, part, at],
        )
        .unwrap();
    }

    #[test]
    fn a_step_s_sales_are_one_event_and_only_what_came_after_the_watermark_counts() {
        let conn = db();
        set_seen(&conn, 100.0);
        record(&conn, 5, "terminal_sale:A", "r1", 90.0);
        record(&conn, 6, "terminal_sale:A", "r2", 110.0);
        record(&conn, 6, "terminal_sale:B", "r3", 110.0);
        record(&conn, 6, "recycling", "r4", 111.0);
        let marks = marks(&conn).unwrap();
        assert_eq!(marks.seen_through, 100.0);
        assert_eq!(
            marks.unseen, 2,
            "the step's two sales are one event, its recycling another"
        );
    }

    #[test]
    fn the_watermark_moves_forward_only_and_never_past_now() {
        let conn = db();
        set_seen(&conn, 100.0);
        assert!(!mark_seen(&conn, 50.0, 200.0).unwrap(), "never back");
        assert!(mark_seen(&conn, 150.0, 200.0).unwrap());
        assert_eq!(marks(&conn).unwrap().seen_through, 150.0);
        assert!(mark_seen(&conn, 900.0, 200.0).unwrap());
        assert_eq!(marks(&conn).unwrap().seen_through, 200.0, "never past now");
        assert!(!mark_seen(&conn, f64::NAN, 300.0).unwrap());
    }

    #[test]
    fn a_listing_outcome_counts_while_it_stands() {
        let conn = db();
        set_seen(&conn, 100.0);
        conn.execute(
            "INSERT INTO auction_listings (id, item_name, quantity, tt_value, starting_bid, \
                 listed_at, created_at, updated_at, status) \
             VALUES ('l1', 'Item', 1, 1.0, 1.0, '2026-06-01', 1, 1, 'sold')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO listing_outcomes_from_account \
                 (listing_id, outcome, reading_id, prev_reading_id, seen_from, seen_to, created_at) \
             VALUES ('l1', 'sold', 7, 6, 0, 1, 120)",
            [],
        )
        .unwrap();
        assert_eq!(marks(&conn).unwrap().unseen, 1);
        // Taken back since: the history no longer shows it as the account's.
        conn.execute(
            "UPDATE auction_listings SET status = 'pending' WHERE id = 'l1'",
            [],
        )
        .unwrap();
        assert_eq!(marks(&conn).unwrap().unseen, 0);
    }
}
