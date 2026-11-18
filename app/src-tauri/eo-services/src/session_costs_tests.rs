//! The cost resolver: a bucket the account readings measured is the
//! session's cost, shared out over its tracked events; anything else keeps
//! its tracked figure and says why; and every figure derived from a
//! session's costs follows its measured costs when they arrive.
//!
//! The reconciliation property (`cycled_reconciliation_tests`) checks the
//! resolved buckets across every read that reports Cycled; these pin the
//! rule itself, the stored vocabulary, freshness, the expected figures and
//! the parts below a session.

use std::sync::Arc;

use rusqlite::Connection;

use crate::db::Db;
use crate::session_costs::{
    self, Basis, Bucket, BucketCosts, CostBasis, StoredBucket, TrackedReason,
};

/// The version of stored costs the resolver reads.
const CURRENT: i64 = session_costs::MEASURED_COSTS_VERSION;

fn conn() -> Connection {
    crate::db::migrated_in_memory().unwrap()
}

/// An ended session's own-grain tracked costs.
fn session(conn: &Connection, id: &str, armour: f64, heal: f64, consumable: f64, dangling: f64) {
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, armour_cost, \
         heal_cost, consumable_cost, dangling_cost, session_name) \
         VALUES (?1, 1000.0, 4600.0, 0, ?2, ?3, ?4, ?5, 'Run')",
        rusqlite::params![id, armour, heal, consumable, dangling],
    )
    .unwrap();
}

/// A kill with one weapon phase: `(tool, shots, cost per shot)`.
fn kill(
    conn: &Connection,
    id: &str,
    session: &str,
    at: f64,
    (tool, shots, per_shot): (&str, i64, f64),
    enhancer: f64,
    loot: f64,
) {
    conn.execute(
        "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
         cost_ped, enhancer_cost, loot_total_ped) \
         VALUES (?1, ?2, 'Young Atrox', 'Atrox', 'Young', ?3, ?4, ?5, ?6)",
        rusqlite::params![id, session, at, shots as f64 * per_shot, enhancer, loot],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, tool, shots, per_shot],
    )
    .unwrap();
}

fn swing(conn: &Connection, id: &str, session: &str, cost: f64) {
    conn.execute(
        "INSERT INTO harvest_events (id, session_id, timestamp, success, tool_name, yield_tier, \
         cost_ped, loot_total_ped) VALUES (?1, ?2, 2000.0, 1, 'Cutter', 'short', ?3, 0.1)",
        rusqlite::params![id, session, cost],
    )
    .unwrap();
}

/// A bucket's stored measured cost: an amount in PED, measured but for the
/// steps the ammo was topped up in (the weapon), or unmeasured for a reason.
#[derive(Clone, Copy)]
enum Stored {
    Measured(f64),
    Mixed(f64),
    Unmeasured(&'static str),
}

/// Store a session's measured costs as the account side does at seal: a
/// header of `costs_version`, and the named buckets; every other bucket is
/// left unmeasured for a gap.
fn store(conn: &Connection, id: &str, costs_version: i64, buckets: &[(&str, Stored)]) {
    conn.execute(
        "INSERT INTO account_session_costs (session_id, costs_version, measure_version, \
         measure_computed_at, status, reason, computed_at) \
         VALUES (?1, ?2, ?3, 1.0, 'measured', NULL, 2.0)",
        rusqlite::params![id, costs_version, session_costs::MEASURED_MEASURE_VERSION],
    )
    .unwrap();
    for bucket in [
        "weapon",
        "enhancer",
        "harvesting",
        "healing",
        "armour",
        "consumables",
        "travel",
        "fieldLogistics",
        "unclassified",
    ] {
        let stored = buckets
            .iter()
            .find(|(name, _)| *name == bucket)
            .map_or(Stored::Unmeasured("gap"), |(_, stored)| *stored);
        let (basis, reason, amount, precision) = match stored {
            Stored::Measured(ped) => (
                "measured",
                None,
                Some((ped * 1_000_000.0).round() as i64),
                Some("exact"),
            ),
            Stored::Mixed(ped) => (
                "mixed",
                None,
                Some((ped * 1_000_000.0).round() as i64),
                Some("exact"),
            ),
            Stored::Unmeasured(reason) => ("unmeasured", Some(reason), None, None),
        };
        conn.execute(
            "INSERT INTO account_session_cost_buckets \
             (session_id, bucket, basis, reason, amount_micro_ped, precision) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![id, bucket, basis, reason, amount, precision],
        )
        .unwrap();
    }
}

/// Store a session's measured costs, the named buckets measured at the
/// given PED and every other left unmeasured: for tests elsewhere that
/// need a measured session without naming the stored costs.
pub(crate) fn store_measured(conn: &Connection, id: &str, buckets: &[(&str, f64)]) {
    let buckets: Vec<(&str, Stored)> = buckets
        .iter()
        .map(|(bucket, ped)| (*bucket, Stored::Measured(*ped)))
        .collect();
    store(conn, id, session_costs::MEASURED_COSTS_VERSION, &buckets);
}

fn resolve(conn: &Connection, id: &str) -> session_costs::SessionCosts {
    session_costs::refresh(conn, id).unwrap();
    session_costs::resolve(conn, id).unwrap()
}

fn assert_near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

/// Weapon 2.00 over two kills, enhancer 0.10, dangling 0.50, armour 0.40,
/// healing 0.30, consumed doses 0.20, harvesting 0.60 over two swings.
fn hunted(conn: &Connection, id: &str) {
    session(conn, id, 0.4, 0.3, 0.2, 0.5);
    kill(
        conn,
        &format!("{id}-k1"),
        id,
        1500.0,
        ("Rifle", 20, 0.05),
        0.1,
        1.0,
    );
    kill(
        conn,
        &format!("{id}-k2"),
        id,
        1600.0,
        ("Pistol", 10, 0.1),
        0.0,
        2.0,
    );
    swing(conn, &format!("{id}-h1"), id, 0.25);
    swing(conn, &format!("{id}-h2"), id, 0.35);
}

// ── The stored vocabulary ───────────────────────────────────────────

/// The values a table's CHECK allows a column, read from the schema.
fn allowed(conn: &Connection, table: &str, column: &str) -> Vec<String> {
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .unwrap();
    let start = sql
        .find(&format!("{column} "))
        .and_then(|at| sql[at..].find(" IN (").map(|open| at + open + 5))
        .unwrap_or_else(|| panic!("{table}.{column} has a closed set"));
    let end = start + sql[start..].find(')').unwrap();
    sql[start..end]
        .split(',')
        .map(|value| value.trim().trim_matches('\'').to_string())
        .collect()
}

#[test]
fn every_value_the_stored_costs_allow_is_one_the_resolver_knows() {
    let conn = conn();
    for bucket in allowed(&conn, "account_session_cost_buckets", "bucket") {
        assert!(StoredBucket::parse(&bucket).is_some(), "{bucket}");
    }
    // The one reason the schema confines to the overhead buckets, which the
    // resolver passes over, is the only one it need not know.
    for reason in allowed(&conn, "account_session_cost_buckets", "reason") {
        if reason == "overheadInGap" {
            continue;
        }
        assert!(TrackedReason::parse(&reason).is_some(), "{reason}");
    }
    let confined = conn.execute(
        "INSERT INTO account_session_cost_buckets VALUES \
           ('s', 'weapon', 'unmeasured', 'overheadInGap', NULL, NULL)",
        [],
    );
    assert!(confined.is_err(), "a Cycled bucket never carries it");
    let buckets = allowed(&conn, "session_cost_resolution_buckets", "bucket");
    assert_eq!(
        buckets,
        Bucket::ALL
            .map(|bucket| bucket.as_str().to_string())
            .to_vec()
    );
    let reasons = allowed(&conn, "session_cost_resolution_buckets", "reason");
    assert_eq!(
        reasons,
        TrackedReason::STORED
            .map(|reason| reason.as_str().to_string())
            .to_vec()
    );
    assert_eq!(StoredBucket::parse("weapons"), None);
    assert_eq!(TrackedReason::parse("stillRunning"), None);
    // The armour parts the resolver sums: a new part would need reading.
    assert_eq!(
        allowed(&conn, "account_session_cost_armour_parts", "part"),
        ["unlimited", "limitedSet", "unconfirmed"]
    );
}

#[test]
fn a_stored_value_the_resolver_cannot_read_leaves_that_session_tracked_and_the_rest_serving() {
    let conn = conn();
    hunted(&conn, "bad");
    hunted(&conn, "good");
    for id in ["bad", "good"] {
        store(&conn, id, CURRENT, &[("weapon", Stored::Measured(3.0))]);
    }
    conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    conn.execute(
        "UPDATE account_session_cost_buckets SET bucket = 'ammunition' \
         WHERE session_id = 'bad' AND bucket = 'weapon'",
        [],
    )
    .unwrap();
    session_costs::heal(&conn).unwrap();
    let bad = session_costs::resolve(&conn, "bad").unwrap();
    assert_eq!(bad.resolved, bad.tracked);
    assert_eq!(
        bad.basis(Bucket::Weapon),
        Basis::Tracked(TrackedReason::NoMeasuredCosts)
    );
    // It is not taken up again until its costs change.
    assert!(session_costs::out_of_step(&conn).unwrap().is_empty());
    let good = session_costs::resolve(&conn, "good").unwrap();
    assert_eq!(good.basis(Bucket::Weapon), Basis::Measured(3.0));

    // A negative amount, or a kept row out of its closed set, reads the
    // same way.
    conn.execute(
        "UPDATE account_session_cost_buckets SET bucket = 'weapon', \
         amount_micro_ped = -1 WHERE session_id = 'bad' AND bucket = 'ammunition'",
        [],
    )
    .unwrap();
    session_costs::refresh(&conn, "bad").unwrap();
    let bad = session_costs::resolve(&conn, "bad").unwrap();
    assert_eq!(bad.resolved, bad.tracked);
    conn.execute(
        "UPDATE session_cost_resolution_buckets SET basis = 'estimated' \
         WHERE session_id = 'good'",
        [],
    )
    .unwrap();
    let good = session_costs::resolve(&conn, "good").unwrap();
    assert_eq!(good.resolved, good.tracked);
}

// ── The rule ────────────────────────────────────────────────────────

#[test]
fn a_session_without_measured_costs_reads_as_tracked() {
    let conn = conn();
    hunted(&conn, "s1");
    let costs = resolve(&conn, "s1");
    let tracked = BucketCosts {
        weapon: 2.0,
        enhancer: 0.1,
        armour: 0.4,
        heal: 0.3,
        consumable: 0.2,
        dangling: 0.5,
        harvest: 0.6,
    };
    assert_eq!(costs.tracked, tracked);
    assert_eq!(costs.resolved, tracked);
    for bucket in Bucket::ALL {
        assert_eq!(
            costs.basis(bucket),
            Basis::Tracked(TrackedReason::NoMeasuredCosts)
        );
        assert_eq!(costs.scale(bucket), 1.0);
    }
    assert_eq!(costs.cycled_basis(), CostBasis::Tracked);
}

#[test]
fn the_one_bucket_sum_is_every_bucket_in_one_order() {
    let costs = BucketCosts {
        weapon: 1.0,
        enhancer: 2.0,
        armour: 4.0,
        heal: 8.0,
        consumable: 16.0,
        dangling: 32.0,
        harvest: 64.0,
    };
    assert_eq!(costs.cycled(), 127.0);
}

#[test]
fn a_measured_weapon_bucket_is_shared_over_the_kills_and_the_dangling_spend() {
    let conn = conn();
    hunted(&conn, "s1");
    // Tracked: 2.00 on kills and 0.50 dangling; measured 3.00.
    store(&conn, "s1", CURRENT, &[("weapon", Stored::Measured(3.0))]);
    let costs = resolve(&conn, "s1");
    assert_eq!(costs.basis(Bucket::Weapon), Basis::Measured(3.0));
    assert_near(costs.scale(Bucket::Weapon), 1.2);
    assert_near(costs.resolved.weapon, 2.4);
    assert_near(costs.resolved.dangling, 0.6);
    assert_near(costs.resolved.weapon + costs.resolved.dangling, 3.0);
    // Nothing else moves.
    assert_eq!(costs.resolved.enhancer, 0.1);
    assert_eq!(costs.resolved.armour, 0.4);
    assert_near(costs.resolved.cycled(), costs.tracked.cycled() + 0.5);
    // The other buckets were left unmeasured for a gap.
    assert_eq!(
        costs.basis(Bucket::Armour),
        Basis::Tracked(TrackedReason::Gap)
    );
    assert_eq!(costs.cycled_basis(), CostBasis::Mixed);
}

#[test]
fn measured_session_grain_buckets_replace_the_session_figures() {
    let conn = conn();
    hunted(&conn, "s1");
    store(
        &conn,
        "s1",
        CURRENT,
        &[
            ("weapon", Stored::Measured(2.5)),
            ("enhancer", Stored::Measured(0.1)),
            ("harvesting", Stored::Measured(0.9)),
            ("healing", Stored::Measured(0.45)),
            ("armour", Stored::Measured(0.33)),
            ("consumables", Stored::Measured(0.2)),
        ],
    );
    let costs = resolve(&conn, "s1");
    assert_eq!(costs.resolved.heal, 0.45);
    assert_eq!(costs.resolved.armour, 0.33);
    assert_eq!(costs.resolved.consumable, 0.2);
    assert_near(costs.resolved.harvest, 0.9);
    assert_near(costs.scale(Bucket::Harvesting), 1.5);
    assert_near(costs.resolved.cycled(), 2.5 + 0.1 + 0.9 + 0.45 + 0.33 + 0.2);
    assert_eq!(costs.cycled_basis(), CostBasis::Measured);
    for bucket in Bucket::ALL {
        assert_eq!(costs.cost_basis(bucket), CostBasis::Measured, "{bucket:?}");
    }
}

#[test]
fn a_measured_bucket_no_tracked_event_carries_sits_at_the_session_grain() {
    let conn = conn();
    session(&conn, "s1", 0.0, 0.0, 0.0, 0.0);
    store(
        &conn,
        "s1",
        CURRENT,
        &[
            ("weapon", Stored::Measured(1.25)),
            ("harvesting", Stored::Measured(0.5)),
        ],
    );
    let costs = resolve(&conn, "s1");
    assert_eq!(costs.resolved.weapon, 1.25);
    assert_eq!(costs.resolved.dangling, 0.0);
    assert_eq!(costs.resolved.harvest, 0.5);
    // No event to carry it: nothing below the session weighs differently.
    assert_eq!(costs.scale(Bucket::Weapon), 1.0);
    assert_eq!(costs.scale(Bucket::Harvesting), 1.0);
}

#[test]
fn a_bucket_measured_at_nothing_is_nothing_whatever_was_tracked() {
    let conn = conn();
    hunted(&conn, "s1");
    store(
        &conn,
        "s1",
        CURRENT,
        &[
            ("harvesting", Stored::Measured(0.0)),
            ("consumables", Stored::Measured(0.0)),
        ],
    );
    let costs = resolve(&conn, "s1");
    assert_eq!(costs.basis(Bucket::Harvesting), Basis::Measured(0.0));
    assert_eq!(costs.resolved.harvest, 0.0);
    assert_eq!(costs.scale(Bucket::Harvesting), 0.0);
    assert_eq!(costs.basis(Bucket::Consumables), Basis::Measured(0.0));
    assert_eq!(costs.resolved.consumable, 0.0);
}

#[test]
fn an_unmeasured_bucket_keeps_its_tracked_figure_and_says_why() {
    let conn = conn();
    hunted(&conn, "s1");
    store(
        &conn,
        "s1",
        CURRENT,
        &[
            ("weapon", Stored::Measured(2.5)),
            ("armour", Stored::Unmeasured("limitedSetUnconfirmed")),
            ("enhancer", Stored::Unmeasured("trackedOnly")),
        ],
    );
    let costs = resolve(&conn, "s1");
    assert_eq!(costs.resolved.armour, 0.4);
    assert_eq!(
        costs.basis(Bucket::Armour),
        Basis::Tracked(TrackedReason::LimitedSetUnconfirmed)
    );
    assert_eq!(
        costs.basis(Bucket::Enhancer),
        Basis::Tracked(TrackedReason::TrackedOnly)
    );
    assert_eq!(costs.cycled_basis(), CostBasis::Mixed);
}

#[test]
fn a_bucket_holding_nothing_leaves_the_session_basis_alone() {
    let conn = conn();
    session(&conn, "s1", 0.0, 0.0, 0.0, 0.0);
    kill(&conn, "k1", "s1", 1500.0, ("Rifle", 10, 0.1), 0.0, 1.0);
    // Only the weapon holds anything, and it is measured: the enhancer's
    // tracked nothing does not make the session mixed.
    store(&conn, "s1", CURRENT, &[("weapon", Stored::Measured(1.1))]);
    assert_eq!(resolve(&conn, "s1").cycled_basis(), CostBasis::Measured);
}

#[test]
fn stored_costs_of_other_rules_resolve_to_tracked() {
    let conn = conn();
    hunted(&conn, "s1");
    store(
        &conn,
        "s1",
        session_costs::MEASURED_COSTS_VERSION + 1,
        &[("weapon", Stored::Measured(3.0))],
    );
    let costs = resolve(&conn, "s1");
    assert_eq!(costs.resolved, costs.tracked);
    assert_eq!(
        costs.basis(Bucket::Weapon),
        Basis::Tracked(TrackedReason::NoMeasuredCosts)
    );
}

#[test]
fn overhead_never_enters_cycled() {
    let conn = conn();
    hunted(&conn, "s1");
    store(
        &conn,
        "s1",
        CURRENT,
        &[
            ("travel", Stored::Measured(5.0)),
            ("fieldLogistics", Stored::Measured(1.0)),
            ("unclassified", Stored::Measured(2.0)),
        ],
    );
    let costs = resolve(&conn, "s1");
    assert_eq!(costs.resolved, costs.tracked);
    // It is kept beside Cycled, by category, and the session's Costs are
    // both.
    assert_eq!(
        costs.overhead,
        session_costs::Overhead {
            travel: 5.0,
            field_logistics: 1.0,
            unclassified: 2.0,
        }
    );
    assert_near(costs.costs(), costs.resolved.cycled() + 8.0);
}

#[test]
fn overhead_is_measured_only() {
    let conn = conn();
    // No measured costs: no overhead, whatever was tracked.
    hunted(&conn, "none");
    assert_eq!(
        resolve(&conn, "none").overhead,
        session_costs::Overhead::default()
    );
    // An overhead category left unmeasured has none, and says nothing; the
    // measured ones stand.
    hunted(&conn, "part");
    store(
        &conn,
        "part",
        CURRENT,
        &[
            ("travel", Stored::Unmeasured("overheadInGap")),
            ("fieldLogistics", Stored::Measured(0.25)),
        ],
    );
    let part = resolve(&conn, "part");
    assert_eq!(part.overhead.travel, 0.0);
    assert_eq!(part.overhead.field_logistics, 0.25);
    assert_eq!(part.overhead.unclassified, 0.0);
    // Stored by other rules, the session reads as never measured.
    hunted(&conn, "old");
    store(
        &conn,
        "old",
        CURRENT - 1,
        &[("travel", Stored::Measured(4.0))],
    );
    assert_eq!(
        resolve(&conn, "old").overhead,
        session_costs::Overhead::default()
    );
    // A kept overhead out of range reads as unreadable, never as a figure.
    conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    conn.execute(
        "UPDATE session_cost_resolutions SET overhead_field_logistics_ped = -1 \
         WHERE session_id = 'part'",
        [],
    )
    .unwrap();
    let unreadable = session_costs::resolve(&conn, "part").unwrap();
    assert_eq!(unreadable.overhead, session_costs::Overhead::default());
    assert_eq!(unreadable.resolved, unreadable.tracked);
}

#[test]
fn the_overhead_sql_sums_what_the_resolution_keeps() {
    let conn = conn();
    hunted(&conn, "a");
    hunted(&conn, "b");
    hunted(&conn, "c");
    store(
        &conn,
        "a",
        CURRENT,
        &[
            ("travel", Stored::Measured(0.5)),
            ("unclassified", Stored::Measured(0.25)),
        ],
    );
    store(&conn, "b", CURRENT, &[("travel", Stored::Measured(1.0))]);
    for id in ["a", "b", "c"] {
        session_costs::refresh(&conn, id).unwrap();
    }
    let (travel, field, unclassified): (Option<f64>, Option<f64>, Option<f64>) = conn
        .query_row(
            &format!(
                "SELECT {} FROM tracking_sessions s",
                session_costs::overhead_sums_sql("s")
            ),
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((travel, field, unclassified), (Some(1.5), None, Some(0.25)));
    for (id, total) in [("a", 0.75), ("b", 1.0), ("c", 0.0)] {
        let sql_total: f64 = conn
            .query_row(
                &format!("SELECT {}", session_costs::overhead_total_sql("?1")),
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_near(sql_total, total);
        assert_near(resolve(&conn, id).overhead.total(), total);
    }
}

// ── Freshness ───────────────────────────────────────────────────────

#[test]
fn a_resolution_kept_by_other_rules_or_from_other_costs_is_out_of_step() {
    let conn = conn();
    hunted(&conn, "s1");
    store(&conn, "s1", CURRENT, &[("weapon", Stored::Measured(3.0))]);
    // Stored costs no resolution has followed yet.
    assert_eq!(session_costs::out_of_step(&conn).unwrap(), vec!["s1"]);
    session_costs::refresh(&conn, "s1").unwrap();
    assert!(session_costs::out_of_step(&conn).unwrap().is_empty());

    // Kept by other rules: out of step, and read as absent until healed.
    conn.execute(
        "UPDATE session_cost_resolutions SET resolution_version = ?1",
        [session_costs::RESOLUTION_VERSION + 1],
    )
    .unwrap();
    assert_eq!(session_costs::out_of_step(&conn).unwrap(), vec!["s1"]);
    let stale = session_costs::resolve(&conn, "s1").unwrap();
    assert_eq!(stale.resolved, stale.tracked);
    session_costs::refresh(&conn, "s1").unwrap();
    assert_eq!(
        session_costs::resolve(&conn, "s1")
            .unwrap()
            .basis(Bucket::Weapon),
        Basis::Measured(3.0)
    );

    // The costs were computed again since: the same.
    conn.execute("UPDATE account_session_costs SET computed_at = 3.0", [])
        .unwrap();
    assert_eq!(session_costs::out_of_step(&conn).unwrap(), vec!["s1"]);
    let stale = session_costs::resolve(&conn, "s1").unwrap();
    assert_eq!(stale.resolved, stale.tracked);
    session_costs::refresh(&conn, "s1").unwrap();
    assert!(session_costs::out_of_step(&conn).unwrap().is_empty());

    // The costs went: the resolution follows them out.
    for table in ["account_session_cost_buckets", "account_session_costs"] {
        conn.execute(&format!("DELETE FROM {table}"), []).unwrap();
    }
    assert_eq!(session_costs::out_of_step(&conn).unwrap(), vec!["s1"]);
    session_costs::heal(&conn).unwrap();
    assert!(session_costs::out_of_step(&conn).unwrap().is_empty());
    assert_eq!(resolve(&conn, "s1").resolved, resolve(&conn, "s1").tracked);
}

async fn open() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("entropia_orme.db"))
        .await
        .unwrap();
    (dir, db)
}

fn now() -> f64 {
    // A day after the sessions, so their days are rolled.
    4600.0 + 2.0 * 86400.0
}

#[tokio::test]
async fn measured_costs_stored_after_the_session_heal_every_figure_derived_from_it() {
    let (_dir, db) = open().await;
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO session_definitions (id, name, ad_hoc_segments, is_active) \
             VALUES (7, 'Dailies', 0, 1)",
            [],
        )?;
        hunted(conn, "s1");
        conn.execute("UPDATE tracking_sessions SET definition_id = 7", [])?;
        crate::session_summary::write_session_summary(conn, "s1")?;
        crate::session_rollup::recompute_session(conn, "s1")?;
        Ok(())
    })
    .await
    .unwrap();
    let clock = Arc::new(crate::clock::MockClock::new(
        chrono::DateTime::from_timestamp(now() as i64, 0).map(|at| at.naive_utc()),
        0.0,
    ));
    let analytics = crate::analytics::AnalyticsService::new(db.clone(), clock.clone());
    let before = analytics.overview("all").await.unwrap();
    let detail_before = crate::tracking_reads::get_session_impl(&db, "s1", now())
        .await
        .unwrap()
        .unwrap();
    assert_near(before.losses_breakdown.tracking_cost, 4.1);

    // The readings' costs arrive after the summary and the days were
    // written: weapon 3.00 against 2.50 tracked, armour 1.00 against 0.40.
    db.with_writer(|conn| {
        store(
            conn,
            "s1",
            CURRENT,
            &[
                ("weapon", Stored::Measured(3.0)),
                ("armour", Stored::Measured(1.0)),
            ],
        );
        Ok(())
    })
    .await
    .unwrap();

    // Every heal catches it up: the Overview (days), Activity and the list
    // (summaries), Hunting (cells).
    let after = analytics.overview("all").await.unwrap();
    assert_near(after.losses_breakdown.tracking_cost, 4.1 + 0.5 + 0.6);
    let lines = &after.losses_breakdown.cycled_breakdown;
    assert_near(lines.weapon.as_f64(), 2.4);
    assert_near(lines.dangling.as_f64(), 0.6);
    assert_near(lines.armour.as_f64(), 1.0);
    // Loot is untouched.
    assert_eq!(
        after.returns_breakdown.loot_tt,
        before.returns_breakdown.loot_tt
    );
    let timeline: f64 = after.timeline.iter().map(|day| day.tracking_cost).sum();
    assert_near(timeline, 5.2);

    let activity = analytics.hunting().await.unwrap();
    let run = activity
        .name_comparisons
        .iter()
        .find(|row| row.name == "Run")
        .unwrap();
    assert_near(run.cycled, 5.2);

    let page = crate::tracking_reads::list_sessions_impl(&db, now(), None, None, None)
        .await
        .unwrap();
    let row = &page.sessions.as_array().unwrap()[0];
    assert_near(row["cost"].as_f64().unwrap(), 5.2);

    let detail = crate::tracking_reads::get_session_impl(&db, "s1", now())
        .await
        .unwrap()
        .unwrap();
    let summary = &detail["summary"];
    assert_near(summary["cost"].as_f64().unwrap(), 5.2);
    // Net moves by exactly the difference; the returns do not move.
    assert_eq!(summary["returns"], detail_before["summary"]["returns"]);
    assert_near(
        summary["net"].as_f64().unwrap(),
        detail_before["summary"]["net"].as_f64().unwrap() - 1.1,
    );
    assert_eq!(summary["costBasis"], "mixed");
    let breakdown = &summary["costBreakdown"];
    // The measured weapon is shown whole (its dangling share included),
    // so the lines add up to the session's cost.
    assert_near(breakdown["weaponCost"].as_f64().unwrap(), 3.0);
    let lines: f64 = [
        "weaponCost",
        "healCost",
        "consumableCost",
        "enhancerCost",
        "armourCost",
        "harvestCost",
    ]
    .iter()
    .map(|line| breakdown[*line].as_f64().unwrap())
    .sum();
    assert_near(lines, 5.2);
    // Beside it, what the chat log tracked: every shot, the kills' 2.00
    // and the 0.50 after the last kill.
    assert_near(breakdown["tracked"]["weaponCost"].as_f64().unwrap(), 2.5);
    assert_near(breakdown["tracked"]["armourCost"].as_f64().unwrap(), 0.4);
    assert_near(breakdown["armourCost"].as_f64().unwrap(), 1.0);
    assert_eq!(breakdown["basis"]["weaponCost"], "measured");
    assert_eq!(breakdown["basis"]["armourCost"], "measured");
    assert_eq!(breakdown["basis"]["healCost"], "tracked");
    // Each weapon's part is its tracked cost at the weapon's scale.
    let tools = detail["toolStats"].as_array().unwrap();
    let parts: f64 = tools
        .iter()
        .map(|tool| tool["costAttributed"].as_f64().unwrap())
        .sum();
    assert_near(parts, 2.4);

    // Hunting's kill grain is the kills' resolved weapon and enhancer.
    let hunting = analytics.hunting_activity("all").await.unwrap();
    assert_near(hunting.overall.cycled, 2.4 + 0.1);

    // The definition's lifetime figures and the skilling forecast read the
    // healed summary.
    let definitions = crate::session_definitions::SessionDefinitionService::new(db.clone(), clock);
    let lifetime = definitions.lifetime_stats(7).await.unwrap();
    assert_near(lifetime.cycled, 5.2);
    assert_near(lifetime.weapon_cost, 2.4);
    let forecast = crate::skilling_forecast::load_forecast_sessions(&db)
        .await
        .unwrap();
    assert_near(forecast[0].cycled_ped, 5.2);
    let weapon_parts: f64 = forecast[0].weapons.iter().map(|(_, ped)| ped).sum();
    assert_near(weapon_parts, 2.4 + 0.1);

    // Nothing is left out of step, and a rebuild from the raw tables
    // agrees with what the heals kept.
    let pending = db
        .with_reader(|conn| Ok(session_costs::out_of_step(conn)?))
        .await
        .unwrap();
    assert!(pending.is_empty());
    let report = crate::maintenance::rebuild_and_verify(&db, now())
        .await
        .unwrap();
    assert!(report.all_matched(), "{:?}", report.tables);
}

/// An enhancer break of the session `session`, carried by `kill` (or by the
/// session, after its last kill), costing `cost` (unpriced when None).
fn enhancer_break(
    conn: &Connection,
    id: &str,
    session: &str,
    kill: Option<&str>,
    cost: Option<f64>,
) {
    let (pricing, tt) = match cost {
        Some(cost) => ("markupNotSet", Some(cost + 0.8)),
        None => ("ttUnknown", None),
    };
    conn.execute(
        "INSERT INTO enhancer_breaks (id, session_id, kill_id, observed_at, enhancer_name, \
         weapon_name, returned_ped, deducted_ped, pricing, tt_ped, cost_ped) \
         VALUES (?1, ?2, ?3, 1700.0, 'Weapon Damage Enhancer 1', 'Rifle', 0.8, 0.8, ?4, ?5, ?6)",
        rusqlite::params![id, session, kill, pricing, tt, cost],
    )
    .unwrap();
}

#[tokio::test]
async fn a_break_after_the_last_kill_is_the_session_s_enhancer_cost_in_every_reader() {
    let (_dir, db) = open().await;
    db.with_writer(|conn| {
        // The hunt's kills carry 0.10 of breaks; after its last kill a
        // break of 0.25, and one of an enhancer of unknown TT.
        hunted(conn, "s1");
        enhancer_break(conn, "b1", "s1", None, Some(0.25));
        enhancer_break(conn, "b2", "s1", None, None);
        crate::session_summary::write_session_summary(conn, "s1")?;
        crate::session_rollup::recompute_session(conn, "s1")?;
        Ok(())
    })
    .await
    .unwrap();

    let tracked = db
        .with_reader(|conn| Ok(session_costs::tracked(conn, "s1")?))
        .await
        .unwrap();
    assert_near(tracked.enhancer, 0.35);
    // Nothing of it is dangling weapon spend.
    assert_near(tracked.dangling, 0.5);

    let clock = Arc::new(crate::clock::MockClock::new(
        chrono::DateTime::from_timestamp(now() as i64, 0).map(|at| at.naive_utc()),
        0.0,
    ));
    let analytics = crate::analytics::AnalyticsService::new(db.clone(), clock);
    let overview = analytics.overview("all").await.unwrap();
    let lines = &overview.losses_breakdown.cycled_breakdown;
    assert_near(lines.enhancer.as_f64(), 0.35);
    assert_near(overview.losses_breakdown.tracking_cost, 4.1 + 0.25);
    let timeline: f64 = overview.timeline.iter().map(|day| day.tracking_cost).sum();
    assert_near(timeline, 4.1 + 0.25);

    let page = crate::tracking_reads::list_sessions_impl(&db, now(), None, None, None)
        .await
        .unwrap();
    assert_near(
        page.sessions.as_array().unwrap()[0]["cost"]
            .as_f64()
            .unwrap(),
        4.1 + 0.25,
    );

    // The session shows both breaks: the priced one in the enhancer
    // bucket, the unpriced one in no figure, with its reason.
    let detail = crate::tracking_reads::get_session_impl(&db, "s1", now())
        .await
        .unwrap()
        .unwrap();
    assert_near(
        detail["summary"]["costBreakdown"]["enhancerCost"]
            .as_f64()
            .unwrap(),
        0.35,
    );
    let breaks = detail["enhancerBreaks"].as_array().unwrap();
    assert_eq!(breaks.len(), 2);
    let unpriced = breaks
        .iter()
        .find(|line| line["pricing"] == "ttUnknown")
        .unwrap();
    assert!(unpriced["costPed"].is_null());
    assert!(unpriced["ttPed"].is_null());

    // A rebuild from the raw tables agrees with what the writes kept.
    let report = crate::maintenance::rebuild_and_verify(&db, now())
        .await
        .unwrap();
    assert!(report.all_matched(), "{:?}", report.tables);

    // Taken out of the statistics, the session's breaks leave with it.
    db.with_writer(|conn| {
        conn.execute(
            "UPDATE tracking_sessions SET economy_excluded_at = 1.0 WHERE id = 's1'",
            [],
        )?;
        session_costs::refresh_derived(conn, "s1")?;
        Ok(())
    })
    .await
    .unwrap();
    let excluded = analytics.overview("all").await.unwrap();
    assert_near(
        excluded.losses_breakdown.cycled_breakdown.enhancer.as_f64(),
        0.0,
    );
    assert_near(excluded.losses_breakdown.tracking_cost, 0.0);
}

#[tokio::test]
async fn a_summary_of_the_rules_before_breaks_were_costed_heals_on_read() {
    let (_dir, db) = open().await;
    db.with_writer(|conn| {
        hunted(conn, "s1");
        enhancer_break(conn, "b1", "s1", None, Some(0.25));
        crate::session_summary::write_session_summary(conn, "s1")?;
        // The row as the earlier rules kept it: no break after the last
        // kill in its enhancer cost or its Cycled.
        conn.execute(
            "UPDATE session_summaries SET summary_version = 9, enhancer_cost = 0.1, \
             cycled_ped = cycled_ped - 0.25 WHERE session_id = 's1'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let page = crate::tracking_reads::list_sessions_impl(&db, now(), None, None, None)
        .await
        .unwrap();
    assert_near(
        page.sessions.as_array().unwrap()[0]["cost"]
            .as_f64()
            .unwrap(),
        4.1 + 0.25,
    );
    let (version, enhancer): (i64, f64) = db
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT summary_version, enhancer_cost FROM session_summaries \
                 WHERE session_id = 's1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(version, crate::session_summary::SUMMARY_VERSION);
    assert_near(enhancer, 0.35);
}

#[tokio::test]
async fn a_session_with_nothing_tracked_and_its_armour_measured_rebuilds_as_kept() {
    let (_dir, db) = open().await;
    db.with_writer(|conn| {
        session(conn, "s1", 0.0, 0.0, 0.0, 0.0);
        store(conn, "s1", CURRENT, &[("armour", Stored::Measured(1.5))]);
        Ok(())
    })
    .await
    .unwrap();
    // The summaries' heal owes it a summary on its measured cost alone.
    let page = crate::tracking_reads::list_sessions_impl(&db, now(), None, None, None)
        .await
        .unwrap();
    assert_near(page.sessions[0]["cost"].as_f64().unwrap(), 1.5);
    let summarised: bool = db
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM session_summaries WHERE session_id = 's1')",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(summarised);
    let report = crate::maintenance::rebuild_and_verify(&db, now())
        .await
        .unwrap();
    assert!(report.all_matched(), "{:?}", report.tables);
}

#[tokio::test]
async fn a_measured_session_excluded_from_the_statistics_reads_as_never_recorded() {
    let (_dir, measured) = open().await;
    let (_dir_without, without) = open().await;
    for (db, with_session) in [(&measured, true), (&without, false)] {
        db.with_writer(move |conn| {
            hunted(conn, "kept");
            conn.execute(
                "UPDATE tracking_sessions SET started_at = 2000.0, ended_at = 5000.0",
                [],
            )?;
            if with_session {
                hunted(conn, "gone");
                store(conn, "gone", CURRENT, &[("weapon", Stored::Measured(9.0))]);
                conn.execute(
                    "UPDATE tracking_sessions SET economy_excluded_at = 1.0 \
                     WHERE id = 'gone'",
                    [],
                )?;
            }
            for id in ["kept", "gone"] {
                session_costs::refresh_derived(conn, id)?;
            }
            Ok(())
        })
        .await
        .unwrap();
    }
    let clock = || {
        Arc::new(crate::clock::MockClock::new(
            chrono::DateTime::from_timestamp(now() as i64, 0).map(|at| at.naive_utc()),
            0.0,
        ))
    };
    let with = crate::analytics::AnalyticsService::new(measured.clone(), clock())
        .overview("all")
        .await
        .unwrap();
    let never = crate::analytics::AnalyticsService::new(without.clone(), clock())
        .overview("all")
        .await
        .unwrap();
    assert_eq!(
        with.losses_breakdown.tracking_cost,
        never.losses_breakdown.tracking_cost
    );
    assert_eq!(with.total_losses, never.total_losses);
}

/// A measured overhead of 0.87: travel 0.50, field logistics 0.25,
/// unclassified 0.12.
const OVERHEAD: [(&str, f64); 3] = [
    ("travel", 0.5),
    ("fieldLogistics", 0.25),
    ("unclassified", 0.12),
];

fn store_overhead(conn: &Connection, id: &str) {
    let buckets: Vec<(&str, Stored)> = OVERHEAD
        .iter()
        .map(|(bucket, ped)| (*bucket, Stored::Measured(*ped)))
        .collect();
    store(conn, id, CURRENT, &buckets);
}

#[tokio::test]
async fn overhead_moves_net_and_costs_by_its_amount_and_nothing_else() {
    let (_dir, db) = open().await;
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO session_definitions (id, name, ad_hoc_segments, is_active) \
             VALUES (7, 'Dailies', 0, 1)",
            [],
        )?;
        hunted(conn, "s1");
        conn.execute("UPDATE tracking_sessions SET definition_id = 7", [])?;
        crate::session_summary::write_session_summary(conn, "s1")?;
        crate::session_rollup::recompute_session(conn, "s1")?;
        Ok(())
    })
    .await
    .unwrap();
    let clock = Arc::new(crate::clock::MockClock::new(
        chrono::DateTime::from_timestamp(now() as i64, 0).map(|at| at.naive_utc()),
        0.0,
    ));
    let analytics = crate::analytics::AnalyticsService::new(db.clone(), clock.clone());
    let definitions =
        crate::session_definitions::SessionDefinitionService::new(db.clone(), clock.clone());
    let read_all = || async {
        let overview = analytics.overview("all").await.unwrap();
        let page = crate::tracking_reads::list_sessions_impl(&db, now(), None, None, None)
            .await
            .unwrap();
        let row = page.sessions.as_array().unwrap()[0].clone();
        let detail = crate::tracking_reads::get_session_impl(&db, "s1", now())
            .await
            .unwrap()
            .unwrap();
        let hunting = analytics.hunting().await.unwrap();
        let run = hunting
            .name_comparisons
            .iter()
            .find(|row| row.name == "Run")
            .map(|row| (row.cycled, row.loot_rate))
            .unwrap();
        let expected: Option<f64> = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT expected_tt_mean FROM session_summaries WHERE session_id = 's1'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        (overview, row, detail, run, expected)
    };
    let (before, row_before, detail_before, run_before, expected_before) = read_all().await;
    let lifetime_before = definitions.lifetime_stats(7).await.unwrap();
    assert_eq!(before.losses_breakdown.overhead.total, 0.0);

    db.with_writer(|conn| {
        store_overhead(conn, "s1");
        Ok(())
    })
    .await
    .unwrap();
    let (after, row, detail, run, expected) = read_all().await;

    // Cycled and every line of it stand; the losses take the overhead.
    assert_eq!(
        after.losses_breakdown.tracking_cost,
        before.losses_breakdown.tracking_cost
    );
    assert_eq!(
        after.losses_breakdown.cycled_breakdown,
        before.losses_breakdown.cycled_breakdown
    );
    let band = &after.losses_breakdown.overhead;
    assert_near(band.total, 0.87);
    assert_near(band.travel, 0.5);
    assert_near(band.field_logistics, 0.25);
    assert_near(band.unclassified, 0.12);
    assert_near(after.total_losses, before.total_losses + 0.87);
    assert_eq!(after.total_gains, before.total_gains);
    assert_eq!(after.returns_breakdown, before.returns_breakdown);
    // The day and month series carry it on the session's start, and agree
    // with the session's own sum; their Cycled does not move.
    let timeline = |overview: &crate::analytics::OverviewData| -> (f64, f64) {
        (
            overview.timeline.iter().map(|day| day.tracking_cost).sum(),
            overview.timeline.iter().map(|day| day.overhead_cost).sum(),
        )
    };
    let (cycled_days, overhead_days) = timeline(&after);
    assert_near(cycled_days, timeline(&before).0);
    assert_near(overhead_days, 0.87);
    let overhead_months: f64 = after
        .monthly_breakdown
        .iter()
        .map(|month| month.overhead_cost)
        .sum();
    assert_near(overhead_months, 0.87);

    // The list and the detail: the cost shown is Cycled and does not move,
    // nor does the return rate; the net moves by exactly the overhead.
    assert_eq!(row["cost"], row_before["cost"]);
    assert_eq!(row["returnRate"], row_before["returnRate"]);
    assert_near(
        row["net"].as_f64().unwrap(),
        row_before["net"].as_f64().unwrap() - 0.87,
    );
    let summary = &detail["summary"];
    let summary_before = &detail_before["summary"];
    assert_eq!(summary["cost"], summary_before["cost"]);
    assert_eq!(summary["returns"], summary_before["returns"]);
    assert_eq!(summary["returnRate"], summary_before["returnRate"]);
    assert_near(
        summary["net"].as_f64().unwrap(),
        summary_before["net"].as_f64().unwrap() - 0.87,
    );
    let breakdown = &summary["costBreakdown"];
    assert_near(breakdown["overheadCost"].as_f64().unwrap(), 0.87);
    assert_near(breakdown["overhead"]["travel"].as_f64().unwrap(), 0.5);
    assert_near(
        breakdown["overhead"]["fieldLogistics"].as_f64().unwrap(),
        0.25,
    );
    assert_near(
        breakdown["overhead"]["unclassified"].as_f64().unwrap(),
        0.12,
    );
    // Activity's Cycled and loot (its Loot-Only Rate) and the expected
    // figure do not move.
    assert_eq!(run, run_before);
    assert_eq!(expected, expected_before);

    // The definition's lifetime takes it beside Cycled, never in it.
    let lifetime = definitions.lifetime_stats(7).await.unwrap();
    assert_eq!(lifetime.cycled, lifetime_before.cycled);
    assert_near(lifetime.overhead, 0.87);

    // Taken out of the statistics, the session's overhead leaves with it,
    // while its own record keeps it.
    db.with_writer(|conn| {
        conn.execute(
            "UPDATE tracking_sessions SET economy_excluded_at = 1.0 WHERE id = 's1'",
            [],
        )?;
        session_costs::refresh_derived(conn, "s1")?;
        Ok(())
    })
    .await
    .unwrap();
    let excluded = analytics.overview("all").await.unwrap();
    assert_eq!(excluded.losses_breakdown.overhead.total, 0.0);
    assert_eq!(excluded.total_losses, 0.0);
    let (_, overhead_days) = timeline(&excluded);
    assert_eq!(overhead_days, 0.0);
    let own = crate::tracking_reads::get_session_impl(&db, "s1", now())
        .await
        .unwrap()
        .unwrap();
    assert_near(
        own["summary"]["costBreakdown"]["overheadCost"]
            .as_f64()
            .unwrap(),
        0.87,
    );

    // A rebuild from the raw tables agrees with what the heals kept.
    let report = crate::maintenance::rebuild_and_verify(&db, now())
        .await
        .unwrap();
    assert!(report.all_matched(), "{:?}", report.tables);
}

/// One stored measured line of the session `id`: its bucket, category,
/// kind and item; its units; its amount and precision; and its markup or
/// the reason it is charged nothing.
fn store_line(
    conn: &Connection,
    id: &str,
    position: i64,
    (bucket, category, kind, item): (&str, &str, &str, &str),
    units: Option<i64>,
    amount: Option<(f64, &str)>,
    (markup, reason): (Option<i64>, Option<&str>),
) {
    conn.execute(
        "INSERT INTO account_session_cost_lines (session_id, position, bucket, category, kind, \
         item_name, units, amount_micro_ped, precision, markup_hundredths, reason) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            id,
            position,
            bucket,
            category,
            kind,
            item,
            units,
            amount.map(|(ped, _)| (ped * 1_000_000.0).round() as i64),
            amount.map(|(_, precision)| precision),
            markup,
            reason,
        ],
    )
    .unwrap();
}

#[test]
fn the_detail_lists_the_measured_lines_and_marks_a_peg_naming_an_overhead_item() {
    let conn = conn();
    hunted(&conn, "s1");
    store(
        &conn,
        "s1",
        CURRENT,
        &[
            ("weapon", Stored::Measured(3.0)),
            ("healing", Stored::Measured(0.0)),
            ("consumables", Stored::Measured(0.3)),
            ("travel", Stored::Measured(0.11)),
            ("armour", Stored::Unmeasured("limitedSetUnconfirmed")),
        ],
    );
    store_line(
        &conn,
        "s1",
        0,
        ("weapon", "ammo", "ammoBurned", "Universal Ammo"),
        Some(29_800),
        Some((2.98, "exact")),
        (None, None),
    );
    store_line(
        &conn,
        "s1",
        1,
        (
            "weapon",
            "mindforceImplant",
            "decay",
            "NeoPsion 85-B Mindforce Implant (L)",
        ),
        None,
        Some((0.02, "wholePec")),
        (None, None),
    );
    store_line(
        &conn,
        "s1",
        2,
        ("healing", "healingConsumable", "consumed", "Odd Pill"),
        Some(2),
        None,
        (None, Some("boughtThroughLedger")),
    );
    store_line(
        &conn,
        "s1",
        3,
        ("consumables", "consumable", "consumed", "Odd Stim"),
        Some(3),
        Some((0.3, "exact")),
        (Some(15_000), None),
    );
    store_line(
        &conn,
        "s1",
        4,
        ("travel", "mindEssence", "consumed", "Mind Essence"),
        Some(900),
        Some((0.09, "exact")),
        (None, None),
    );
    store_line(
        &conn,
        "s1",
        5,
        ("travel", "teleportChip", "decay", "Teleportation Chip 1"),
        None,
        Some((0.02, "wholePec")),
        (None, None),
    );
    conn.execute_batch(
        "INSERT INTO ledger_entries (id, date, type, description, amount, tag, source_session_id) \
         VALUES ('fuel', '2026-01-02', 'expense', 'Bought MIND ESSENCE for the trip', 1.0, \
                 'misc', 's1'), \
                ('ammo', '2026-01-01', 'expense', 'Universal Ammo top-up', 1.0, 'misc', 's1'), \
                ('sold', '2026-01-01', 'markup', 'Sold Mind Essence', 1.0, 'misc', 's1'), \
                ('elsewhere', '2026-01-01', 'expense', 'Mind Essence', 1.0, 'misc', NULL); \
         INSERT INTO protection_defence_events (session_id, damage, deflected) \
         VALUES ('s1', 4.0, 0);",
    )
    .unwrap();
    session_costs::refresh(&conn, "s1").unwrap();
    let detail = crate::tracking_reads::get_session_read(&conn, "s1", now())
        .unwrap()
        .unwrap();
    let breakdown = &detail["summary"]["costBreakdown"];
    let groups = breakdown["measured"]["buckets"].as_array().unwrap();
    let names: Vec<&str> = groups
        .iter()
        .map(|group| group["bucket"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["weapon", "healing", "consumables", "travel"]);
    let line = |item: &str| -> serde_json::Value {
        groups
            .iter()
            .flat_map(|group| group["lines"].as_array().unwrap())
            .find(|line| line["item"] == item)
            .cloned()
            .unwrap_or_else(|| panic!("a line for {item}"))
    };
    // The implant is its own line, by name and amount.
    let implant = line("NeoPsion 85-B Mindforce Implant (L)");
    assert_eq!(implant["kind"], "decay");
    assert_near(implant["amount"].as_f64().unwrap(), 0.02);
    assert_eq!(implant["precision"], "wholePec");
    // Units at TT of an item bought at markup say the markup is not in.
    let essence = line("Mind Essence");
    assert_eq!(essence["units"], 900);
    assert_eq!(essence["markupNotIncluded"], true);
    assert!(essence["markupPercent"].is_null());
    let stim = line("Odd Stim");
    assert_eq!(stim["markupPercent"], 150.0);
    assert_eq!(stim["markupNotIncluded"], false);
    let pill = line("Odd Pill");
    assert_eq!(pill["unpriced"], "boughtThroughLedger");
    assert!(pill["amount"].is_null());
    // A ledger expense pegged to the session naming an overhead item marks
    // its line, whatever the case; one not pegged, a gain, or one naming a
    // Cycled item marks nothing. Neither figure moves.
    assert_eq!(essence["peggedLedgerEntryId"], "fuel");
    assert_eq!(
        essence["peggedLedgerEntryDescription"],
        "Bought MIND ESSENCE for the trip"
    );
    assert!(line("Teleportation Chip 1")["peggedLedgerEntryDescription"].is_null());
    // A measured overhead category says nothing; a withheld one says why.
    let reasons = &breakdown["overhead"]["reasons"];
    assert!(reasons["travel"].is_null());
    assert_eq!(reasons["fieldLogistics"], "gap");
    assert_eq!(reasons["unclassified"], "gap");
    assert!(line("Teleportation Chip 1")["peggedLedgerEntryId"].is_null());
    assert!(line("Universal Ammo")["peggedLedgerEntryId"].is_null());
    assert_near(breakdown["overhead"]["travel"].as_f64().unwrap(), 0.11);
    // Each bucket says why it is not measured, or nothing.
    let reasons = &breakdown["reasons"];
    assert!(reasons["weaponCost"].is_null());
    assert_eq!(reasons["armourCost"], "limitedSetUnconfirmed");
    assert_eq!(reasons["harvestCost"], "gap");
    assert_eq!(reasons["enhancerCost"], "gap");
    // The hit no recording reaches leaves the tracked armour unrecorded.
    assert_eq!(breakdown["armourUnrecorded"], true);
    // The tracked weapon is every shot fired, as the measured one is.
    assert_near(breakdown["tracked"]["weaponCost"].as_f64().unwrap(), 2.5);

    // A session with no readings has no overhead, no lines and no reasons
    // but the absence of measured costs.
    hunted(&conn, "s2");
    let detail = crate::tracking_reads::get_session_read(&conn, "s2", now())
        .unwrap()
        .unwrap();
    let breakdown = &detail["summary"]["costBreakdown"];
    assert!(breakdown["measured"].is_null());
    assert_eq!(breakdown["overheadCost"], 0.0);
    assert_eq!(breakdown["reasons"]["weaponCost"], "noMeasuredCosts");
    assert_eq!(breakdown["armourUnrecorded"], false);
    for category in ["travel", "fieldLogistics", "unclassified"] {
        assert!(
            breakdown["overhead"]["reasons"][category].is_null(),
            "{category}"
        );
    }
}

#[test]
fn the_detail_prices_each_limited_piece_at_its_own_markup_or_at_its_tt() {
    let conn = conn();
    hunted(&conn, "s1");
    store(&conn, "s1", CURRENT, &[("armour", Stored::Measured(1.546))]);
    conn.execute(
        "INSERT INTO account_session_cost_lines (session_id, position, bucket, category, kind, \
         item_name, units, amount_micro_ped, precision, markup_hundredths, reason, set_id) \
         VALUES ('s1', 0, 'armour', 'limitedArmour', 'limitedDecay', 'Pegasus Helmet (M,L)', \
                 NULL, 330000, 'wholePec', NULL, NULL, NULL), \
                ('s1', 1, 'armour', 'limitedArmour', 'limitedDecay', 'Odd Gloves (L)', \
                 NULL, 20000, 'wholePec', NULL, NULL, NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO account_session_cost_armour_parts (session_id, part, set_id, \
         lost_micro_ped, lost_precision, markup_hundredths, amount_micro_ped, precision) \
         VALUES ('s1', 'unlimited', NULL, 1130000, 'wholePec', NULL, 1130000, 'wholePec')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO account_session_cost_armour_pieces (session_id, item_name, \
         lost_micro_ped, lost_precision, markup_hundredths, amount_micro_ped, precision) \
         VALUES ('s1', 'Pegasus Helmet (M,L)', 330000, 'wholePec', 12000, 396000, 'wholePec'), \
                ('s1', 'Odd Gloves (L)', 20000, 'wholePec', NULL, 20000, 'wholePec')",
        [],
    )
    .unwrap();
    session_costs::refresh(&conn, "s1").unwrap();
    let detail = crate::tracking_reads::get_session_read(&conn, "s1", now())
        .unwrap()
        .unwrap();
    let measured = &detail["summary"]["costBreakdown"]["measured"];
    let lines = measured["buckets"][0]["lines"].as_array().unwrap();
    assert_eq!(measured["buckets"][0]["bucket"], "armour");
    // A piece with a markup set: the TT it lost, at that markup.
    assert_eq!(lines[0]["item"], "Pegasus Helmet (M,L)");
    assert_eq!(lines[0]["markupPercent"], 120.0);
    assert!(lines[0]["unpriced"].is_null());
    assert_near(lines[0]["amount"].as_f64().unwrap(), 0.33);
    // One with none set: at the TT it lost, never waiting.
    assert_eq!(lines[1]["item"], "Odd Gloves (L)");
    assert!(lines[1]["markupPercent"].is_null());
    assert!(lines[1]["unpriced"].is_null());
    let parts = measured["armourParts"].as_array().unwrap();
    let kinds: Vec<&str> = parts
        .iter()
        .map(|part| part["part"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["unlimited", "limitedPiece", "limitedPiece"]);
    // The pieces by name.
    assert_eq!(parts[1]["item"], "Odd Gloves (L)");
    assert!(parts[1]["markupPercent"].is_null());
    assert_near(parts[1]["cost"].as_f64().unwrap(), 0.02);
    let pegasus = &parts[2];
    assert_eq!(pegasus["item"], "Pegasus Helmet (M,L)");
    assert_eq!(pegasus["markupPercent"], 120.0);
    assert_near(pegasus["lost"].as_f64().unwrap(), 0.33);
    assert_near(pegasus["cost"].as_f64().unwrap(), 0.396);
}

#[tokio::test]
async fn a_session_whose_only_cost_is_overhead_is_summarised_and_counted() {
    let (_dir, db) = open().await;
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO session_definitions (id, name, ad_hoc_segments, is_active) \
             VALUES (7, 'Trips', 0, 1)",
            [],
        )?;
        // A trip that fought nothing: no tracked cost, only its teleports.
        session(conn, "trip", 0.0, 0.0, 0.0, 0.0);
        conn.execute("UPDATE tracking_sessions SET definition_id = 7", [])?;
        store_overhead(conn, "trip");
        Ok(())
    })
    .await
    .unwrap();
    let clock = Arc::new(crate::clock::MockClock::new(
        chrono::DateTime::from_timestamp(now() as i64, 0).map(|at| at.naive_utc()),
        0.0,
    ));
    let definitions = crate::session_definitions::SessionDefinitionService::new(db.clone(), clock);
    let lifetime = definitions.lifetime_stats(7).await.unwrap();
    assert_eq!(lifetime.instance_count, 1);
    assert_eq!(lifetime.cycled, 0.0);
    assert_near(lifetime.overhead, 0.87);
    let summarised: (f64, f64) = db
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT cycled_ped, overhead_cost FROM session_summaries \
                 WHERE session_id = 'trip'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(summarised.0, 0.0);
    assert_near(summarised.1, 0.87);
    let forecast = crate::skilling_forecast::load_forecast_sessions(&db)
        .await
        .unwrap();
    assert_eq!(forecast.len(), 1);
    assert_near(forecast[0].overhead_ped, 0.87);
    assert_eq!(forecast[0].cycled_ped, 0.0);
    let page = crate::tracking_reads::list_sessions_impl(&db, now(), None, None, None)
        .await
        .unwrap();
    assert_near(page.sessions[0]["net"].as_f64().unwrap(), -0.87);
    let report = crate::maintenance::rebuild_and_verify(&db, now())
        .await
        .unwrap();
    assert!(report.all_matched(), "{:?}", report.tables);
}

#[test]
fn an_overhead_category_changed_in_a_gap_says_so() {
    let conn = conn();
    hunted(&conn, "s1");
    store(
        &conn,
        "s1",
        CURRENT,
        &[
            ("travel", Stored::Unmeasured("overheadInGap")),
            ("fieldLogistics", Stored::Measured(0.0)),
            ("unclassified", Stored::Measured(0.25)),
        ],
    );
    session_costs::refresh(&conn, "s1").unwrap();
    let detail = crate::tracking_reads::get_session_read(&conn, "s1", now())
        .unwrap()
        .unwrap();
    let overhead = &detail["summary"]["costBreakdown"]["overhead"];
    // Withheld, so never a zero: the reason says why.
    assert_eq!(overhead["reasons"]["travel"], "overheadInGap");
    assert_eq!(overhead["travel"], 0.0);
    // Measured at nothing is a measured nothing.
    assert!(overhead["reasons"]["fieldLogistics"].is_null());
    assert!(overhead["reasons"]["unclassified"].is_null());
    assert_eq!(
        session_costs::overhead_reasons(&conn, "s1").unwrap(),
        [Some(session_costs::OverheadReason::InGap), None, None]
    );
}

#[tokio::test]
async fn activity_overall_overhead_covers_the_sessions_its_cycled_covers() {
    let (_dir, db) = open().await;
    db.with_writer(|conn| {
        // A hunt under no definition, 3.00 of travel.
        session(conn, "hunt", 0.0, 0.0, 0.0, 0.0);
        kill(
            conn,
            "hunt-k",
            "hunt",
            1500.0,
            ("Rifle", 20, 0.05),
            0.0,
            1.0,
        );
        store(conn, "hunt", CURRENT, &[("travel", Stored::Measured(3.0))]);
        // A swing-only session, 0.50 of field logistics.
        session(conn, "swing", 0.0, 0.0, 0.0, 0.0);
        swing(conn, "swing-h", "swing", 0.4);
        store(
            conn,
            "swing",
            CURRENT,
            &[("fieldLogistics", Stored::Measured(0.5))],
        );
        for id in ["hunt", "swing"] {
            session_costs::refresh_derived(conn, id)?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let clock = Arc::new(crate::clock::MockClock::new(
        chrono::DateTime::from_timestamp(now() as i64, 0).map(|at| at.naive_utc()),
        0.0,
    ));
    let analytics = crate::analytics::AnalyticsService::new(db.clone(), clock);
    let hunting = analytics.hunting_activity("all").await.unwrap();
    assert_near(hunting.overall.overhead, 3.0);
    let unassigned = hunting
        .definitions
        .iter()
        .find(|row| row.definition_id.is_none())
        .expect("the unassigned bucket");
    assert_near(unassigned.overhead, 3.0);
    let harvest = analytics.harvest("all").await.unwrap();
    assert_near(harvest.overhead, 0.5);
    let short = harvest
        .tier_comparisons
        .iter()
        .find(|row| row.yield_tier == crate::harvest_yield::HarvestYieldTier::Short)
        .unwrap();
    assert_near(short.overhead, 0.5);
    assert_eq!(harvest.unassigned_overhead, 0.0);

    // Out of the statistics, it leaves the overall with the session.
    db.with_writer(|conn| {
        conn.execute(
            "UPDATE tracking_sessions SET economy_excluded_at = 1.0 WHERE id = 'hunt'",
            [],
        )?;
        session_costs::refresh_derived(conn, "hunt")?;
        Ok(())
    })
    .await
    .unwrap();
    let hunting = analytics.hunting_activity("all").await.unwrap();
    assert_eq!(hunting.overall.overhead, 0.0);
}

#[test]
fn every_resolved_basis_maps_to_one_wire_reason() {
    // The mixed armour reads as waiting for its set; a measured bucket as
    // none; every tracked reason as itself, the weapon topped up (which
    // only the resolver gives, from a mixed weapon) included.
    let conn = conn();
    hunted(&conn, "s1");
    let mut seen = Vec::new();
    for reason in TrackedReason::STORED {
        let mut stored: Vec<(&str, Stored)> = Bucket::ALL
            .iter()
            .map(|bucket| (bucket.as_str(), Stored::Measured(1.0)))
            .collect();
        stored[0] = match reason {
            TrackedReason::AmmoToppedUp => ("weapon", Stored::Mixed(1.0)),
            reason => ("weapon", Stored::Unmeasured(reason.as_str())),
        };
        for table in [
            "session_cost_resolution_weapon",
            "session_cost_resolution_buckets",
            "session_cost_resolutions",
            "account_session_cost_top_ups",
            "account_session_cost_buckets",
            "account_session_costs",
        ] {
            conn.execute(&format!("DELETE FROM {table}"), []).unwrap();
        }
        store(&conn, "s1", CURRENT, &stored);
        if reason == TrackedReason::AmmoToppedUp {
            top_up(&conn, "s1", 1450.0, 1550.0);
        }
        session_costs::refresh(&conn, "s1").unwrap();
        let detail = crate::tracking_reads::get_session_read(&conn, "s1", now())
            .unwrap()
            .unwrap();
        let reasons = &detail["summary"]["costBreakdown"]["reasons"];
        assert_eq!(reasons["weaponCost"], reason.as_str(), "{reason:?}");
        assert!(reasons["healCost"].is_null());
        seen.push(reason);
    }
    assert_eq!(seen.len(), TrackedReason::STORED.len());
}

// ── Expected figures ────────────────────────────────────────────────

#[tokio::test]
async fn an_expected_figure_does_not_move_when_only_measured_costs_change() {
    // Weapon 5.00 tracked with its evidence, and 2.00 of harvesting decay:
    // TT of unknown Efficiency, bounded by what Cycled leaves beside the
    // modelled weapon.
    let expected = |measured: Option<f64>| async move {
        let (_dir, db) = open().await;
        db.with_writer(move |conn| {
            conn.execute_batch(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, mob_tracking_mode) \
                 VALUES ('s1', 1000.0, 8200.0, 0, 'mob'); \
                 INSERT INTO kills (id, session_id, mob_name, timestamp, cost_ped, loot_total_ped) \
                 VALUES ('k1', 's1', 'Atrox', 1500.0, 5.0, 4.0); \
                 INSERT INTO kill_tool_stats \
                     (kill_id, tool_name, shots_fired, cost_per_shot, expected_economics_json) \
                 VALUES ('k1', 'Rifle', 100, 0.05, \
                     '{\"components\":[{\"kind\":\"weapon\",\"name\":\"Rifle\",\
\"efficiencyPct\":80.0,\"rawTtPerUse\":0.05,\"consumedPremiumPerUse\":0.0}],\
\"looters\":{\"animal\":40.0,\"mutant\":50.0,\"robot\":60.0},\
\"looterSource\":\"three_looter_mean\"}');",
            )?;
            swing(conn, "h1", "s1", 2.0);
            if let Some(amount) = measured {
                store(conn, "s1", CURRENT, &[("weapon", Stored::Measured(amount))]);
            }
            crate::session_summary::write_session_summary(conn, "s1")?;
            Ok(conn.query_row(
                "SELECT cycled_ped, expected_tt_mean, expected_tt_animal_high \
                 FROM session_summaries WHERE session_id = 's1'",
                [],
                |row| {
                    Ok((
                        row.get::<_, f64>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                    ))
                },
            )?)
        })
        .await
        .unwrap()
    };
    let (tracked_cycled, tracked_mean, tracked_high) = expected(None).await;
    assert_eq!(tracked_cycled, 7.0);
    assert!(tracked_mean.is_some());
    // Measured above and below what was tracked: Cycled moves, the
    // expected loot does not. Below, a bound on the resolved Cycled would
    // have cut the harvesting decay's share.
    for measured in [5.4, 4.0] {
        let (cycled, mean, high) = expected(Some(measured)).await;
        assert_near(cycled, measured + 2.0);
        assert_eq!(mean, tracked_mean, "{measured}");
        assert_eq!(high, tracked_high, "{measured}");
    }
}

// ── Below the session ───────────────────────────────────────────────

#[test]
fn a_measured_sessions_weapons_add_up_to_its_resolved_kill_spend() {
    let conn = conn();
    hunted(&conn, "s1");
    store(
        &conn,
        "s1",
        CURRENT,
        &[
            ("weapon", Stored::Measured(3.0)),
            ("enhancer", Stored::Measured(0.25)),
        ],
    );
    session_costs::refresh(&conn, "s1").unwrap();
    let cells = crate::session_weapon_rollup::compute(&conn, "s1").unwrap();
    let parts: f64 = cells.weapons.iter().map(|cell| cell.cycled_ped).sum();
    // Weapon 2.00 of 2.50 tracked carries 2.40 of the measured 3.00; the
    // enhancer's one break carries all of its 0.25.
    assert_near(parts, 2.4 + 0.25);
    let rifle = cells
        .weapons
        .iter()
        .find(|cell| cell.tool == "Rifle")
        .unwrap();
    assert_near(rifle.cycled_ped, 1.2 + 0.25);
}

#[test]
fn a_measured_sessions_kill_cells_add_up_to_its_resolved_kill_spend() {
    let conn = conn();
    hunted(&conn, "s1");
    store(&conn, "s1", CURRENT, &[("weapon", Stored::Measured(3.0))]);
    session_costs::refresh(&conn, "s1").unwrap();
    crate::session_rollup::recompute_session(&conn, "s1").unwrap();
    let cycled: f64 = conn
        .query_row(
            "SELECT SUM(cycled_ped) FROM session_kill_rollups WHERE session_id = 's1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_near(cycled, 2.4 + 0.1);
}

#[test]
fn a_contexts_spend_is_its_tracked_spend_at_its_buckets_scales() {
    let conn = conn();
    hunted(&conn, "s1");
    swing(&conn, "s1-h3", "s1", 0.0);
    // Two contexts: the Rifle kill and the first swing in one, the Pistol
    // kill and the second swing in the other.
    for _ in 0..2 {
        conn.execute(
            "INSERT INTO session_contexts (session_id, created_at) VALUES ('s1', 1000.0)",
            [],
        )
        .unwrap();
    }
    conn.execute_batch(
        "UPDATE kills SET context_id = 1 WHERE id = 's1-k1'; \
         UPDATE kills SET context_id = 2 WHERE id = 's1-k2'; \
         UPDATE harvest_events SET context_id = 1 WHERE id = 's1-h1'; \
         UPDATE harvest_events SET context_id = 2 WHERE id = 's1-h2';",
    )
    .unwrap();
    store(
        &conn,
        "s1",
        CURRENT,
        &[
            ("weapon", Stored::Measured(3.0)),
            ("harvesting", Stored::Measured(1.2)),
        ],
    );
    session_costs::refresh(&conn, "s1").unwrap();
    let parts: Vec<f64> = (1..=2)
        .map(|context: i64| {
            conn.query_row(
                &format!(
                    "SELECT COALESCE((SELECT SUM({}) FROM kills k WHERE k.context_id = ?1), 0) \
                          + COALESCE((SELECT SUM({}) FROM harvest_events h \
                                      WHERE h.context_id = ?1), 0)",
                    session_costs::kill_cycled_sql("k"),
                    session_costs::scaled("h.cost_ped", Bucket::Harvesting, "h.session_id"),
                ),
                [context],
                |row| row.get(0),
            )
            .unwrap()
        })
        .collect();
    // Rifle 1.00 at 1.2 with its 0.10 break, swing 0.25 at 2; Pistol 1.00
    // at 1.2, swing 0.35 at 2.
    assert_near(parts[0], 1.2 + 0.1 + 0.5);
    assert_near(parts[1], 1.2 + 0.7);
    assert_near(parts.iter().sum::<f64>(), 2.4 + 0.1 + 1.2);
}

// ── Armour measured in part ─────────────────────────────────────────

/// A limited set at `markup` percent, by id.
fn limited_set(conn: &Connection, name: &str, markup: f64) -> i64 {
    conn.execute(
        "INSERT INTO protection_sets (kind, name, economy_kind, markup_percent, created_at) \
         VALUES ('armour', ?1, 'limited', ?2, 1.0)",
        rusqlite::params![name, markup],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// A recording of `cost` PED that reached `session` whole: a repair, or a
/// limited set's reading.
fn recorded(conn: &Connection, session: &str, set: Option<i64>, cost: f64) -> i64 {
    let (kind, opening, closing) = match set {
        Some(set_id) => {
            let mut ids = Vec::new();
            for tt in [10.0, 9.0] {
                conn.execute(
                    "INSERT INTO protection_observations \
                     (set_id, client_token, tt_value_ped, source, observed_at) \
                     VALUES (?1, lower(hex(randomblob(8))), ?2, 'manual', 1.0)",
                    rusqlite::params![set_id, tt],
                )
                .unwrap();
                ids.push(conn.last_insert_rowid());
            }
            ("limited_decay", Some(ids[0]), Some(ids[1]))
        }
        None => ("repair", None, None),
    };
    conn.execute(
        "INSERT INTO protection_cost_windows \
         (kind, set_id, opening_observation_id, closing_observation_id, consumed_tt_ped, \
          markup_percent, cost_ped, status, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'booked', 1.0)",
        rusqlite::params![
            kind,
            set,
            opening,
            closing,
            set.map(|_| 1.0),
            set.map(|_| 100.0),
            cost
        ],
    )
    .unwrap();
    let window = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO protection_cost_allocations \
         (window_id, session_id, damage_weight, deflection_count, allocation_share, cost_ped) \
         VALUES (?1, ?2, 0, 0, 1, ?3)",
        rusqlite::params![window, session, cost],
    )
    .unwrap();
    window
}

/// The armour parts of a session whose costs [`store`] wrote: unlimited at
/// `unlimited` PED, each `(set, cost)`, and a limited loss no confirmed set
/// holds when `unconfirmed`.
fn store_armour(
    conn: &Connection,
    id: &str,
    unlimited: f64,
    sets: &[(i64, f64)],
    unconfirmed: bool,
) {
    let micro = |ped: f64| (ped * 1_000_000.0).round() as i64;
    conn.execute(
        "INSERT INTO account_session_cost_armour_parts (session_id, part, set_id, \
         lost_micro_ped, lost_precision, markup_hundredths, amount_micro_ped, precision) \
         VALUES (?1, 'unlimited', NULL, ?2, 'wholePec', NULL, ?2, 'wholePec')",
        rusqlite::params![id, micro(unlimited)],
    )
    .unwrap();
    for (set_id, cost) in sets {
        conn.execute(
            "INSERT INTO account_session_cost_armour_parts (session_id, part, set_id, \
             lost_micro_ped, lost_precision, markup_hundredths, amount_micro_ped, precision) \
             VALUES (?1, 'limitedSet', ?2, ?3, 'wholePec', 14000, ?3, 'wholePec')",
            rusqlite::params![id, set_id, micro(*cost)],
        )
        .unwrap();
    }
    if unconfirmed {
        conn.execute(
            "INSERT INTO account_session_cost_armour_parts (session_id, part, set_id, \
             lost_micro_ped, lost_precision, markup_hundredths, amount_micro_ped, precision) \
             VALUES (?1, 'unconfirmed', NULL, 100000, 'wholePec', NULL, NULL, NULL)",
            [id],
        )
        .unwrap();
    }
}

#[test]
fn armour_waiting_for_a_set_s_pieces_is_the_measured_parts_and_that_set_s_recordings() {
    let conn = conn();
    let confirmed = limited_set(&conn, "Pegasus", 140.0);
    let waiting = limited_set(&conn, "Hyperion", 180.0);
    // The tracked armour is the three recordings that reached the session.
    session(&conn, "s1", 0.9, 0.0, 0.0, 0.0);
    recorded(&conn, "s1", None, 0.4);
    recorded(&conn, "s1", Some(confirmed), 0.2);
    let pending = recorded(&conn, "s1", Some(waiting), 0.3);
    store(
        &conn,
        "s1",
        CURRENT,
        &[("armour", Stored::Unmeasured("limitedSetUnconfirmed"))],
    );
    store_armour(&conn, "s1", 1.13, &[(confirmed, 0.462)], true);

    // The repair and the confirmed set's reading are what the readings
    // measured, so they are not added again; the set waiting for its pieces
    // keeps its recording.
    let costs = resolve(&conn, "s1");
    assert_near(costs.resolved.armour, 1.13 + 0.462 + 0.3);
    match costs.basis(Bucket::Armour) {
        Basis::Mixed {
            measured,
            tracked,
            reason,
        } => {
            assert_near(measured, 1.592);
            assert_near(tracked, 0.3);
            assert_eq!(reason, TrackedReason::LimitedSetUnconfirmed);
        }
        other => panic!("not mixed: {other:?}"),
    }
    assert_eq!(costs.cost_basis(Bucket::Armour), CostBasis::Mixed);
    assert_eq!(costs.tracked.armour, 0.9);

    // Every SQL reader sums the same figure.
    let column: f64 = conn
        .query_row(
            &format!(
                "SELECT {} FROM tracking_sessions s WHERE s.id = 's1'",
                session_costs::session_column_sql(session_costs::SessionColumn::Armour, "s")
            ),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_near(column, 1.892);

    // Undoing the waiting set's recording takes it off at the next refresh.
    conn.execute(
        "UPDATE protection_cost_windows SET superseded_at = 2.0 WHERE id = ?1",
        [pending],
    )
    .unwrap();
    let costs = resolve(&conn, "s1");
    assert_near(costs.resolved.armour, 1.592);
    assert_eq!(costs.cost_basis(Bucket::Armour), CostBasis::Mixed);
}

#[test]
fn armour_with_no_measured_part_stays_tracked() {
    let conn = conn();
    session(&conn, "s1", 0.9, 0.0, 0.0, 0.0);
    store(
        &conn,
        "s1",
        CURRENT,
        &[("armour", Stored::Unmeasured("limitedSetUnconfirmed"))],
    );
    let costs = resolve(&conn, "s1");
    assert_eq!(costs.resolved.armour, 0.9);
    assert_eq!(
        costs.basis(Bucket::Armour),
        Basis::Tracked(TrackedReason::LimitedSetUnconfirmed)
    );
}

#[test]
fn the_measured_armour_streams_are_named_by_their_parts() {
    let conn = conn();
    let set = limited_set(&conn, "Pegasus", 140.0);
    let other = limited_set(&conn, "Hyperion", 180.0);
    session(&conn, "s1", 0.0, 0.0, 0.0, 0.0);
    session(&conn, "s2", 0.0, 0.0, 0.0, 0.0);
    store(&conn, "s1", CURRENT, &[]);
    store_armour(&conn, "s1", 1.0, &[(set, 0.5)], false);
    let measured = |stream: session_costs::ArmourStream, id: &str| -> bool {
        conn.query_row(
            &format!(
                "SELECT {}",
                session_costs::armour_measured_sql(stream, "?1")
            ),
            [id],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert!(measured(session_costs::ArmourStream::Unlimited, "s1"));
    assert!(measured(session_costs::ArmourStream::Set(set), "s1"));
    // Read whole with no limited piece unpriced: no other set wore there
    // unmeasured either.
    assert!(measured(session_costs::ArmourStream::Set(other), "s1"));
    assert!(!measured(session_costs::ArmourStream::Unlimited, "s2"));
    // A limited piece no confirmed set holds fell: the sets without a part
    // may have worn, so they are not measured there.
    store(&conn, "s2", CURRENT, &[]);
    store_armour(&conn, "s2", 1.0, &[(set, 0.5)], true);
    assert!(measured(session_costs::ArmourStream::Unlimited, "s2"));
    assert!(measured(session_costs::ArmourStream::Set(set), "s2"));
    assert!(!measured(session_costs::ArmourStream::Set(other), "s2"));
    // Costs stored by other rules measure nothing.
    conn.execute(
        "UPDATE account_session_costs SET costs_version = ?1",
        [CURRENT + 1],
    )
    .unwrap();
    assert!(!measured(session_costs::ArmourStream::Unlimited, "s1"));
}

/// Make a session's stored costs measured across a gap closed by reading
/// 2, after a gap of `gap_seconds`.
fn across_a_gap(conn: &Connection, id: &str, gap_seconds: i64) {
    conn.execute(
        "UPDATE account_session_costs SET status = 'acrossGap', first_reading_id = 1, \
         last_reading_id = 2 WHERE session_id = ?1",
        [id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO account_session_cost_gaps (session_id, closing_reading_id, \
         closing_outside_seconds, closing_gap_seconds) VALUES (?1, 2, 1800, ?2)",
        rusqlite::params![id, gap_seconds],
    )
    .unwrap();
}

/// A session measured across a gap says how long the gap was only where
/// its Cycled rests on the measured figures: one whose Cycled is tracked
/// throughout shows no figure from the gap, so says nothing of it.
#[test]
fn the_gap_is_shown_only_beside_a_figure_measured_across_it() {
    let conn = conn();
    session(&conn, "measured", 0.0, 1.0, 0.0, 2.0);
    store(
        &conn,
        "measured",
        CURRENT,
        &[
            ("weapon", Stored::Measured(2.5)),
            ("healing", Stored::Measured(1.2)),
        ],
    );
    across_a_gap(&conn, "measured", 2_620);
    let costs = resolve(&conn, "measured");
    let gap = Some(session_costs::AcrossGap { gap_seconds: 2_620 });
    assert_eq!(costs.across_gap, gap);
    assert_eq!(costs.shown_across_gap(), gap);

    session(&conn, "tracked", 0.0, 1.0, 0.0, 2.0);
    store(
        &conn,
        "tracked",
        CURRENT,
        &[("weapon", Stored::Unmeasured("ammoUnsettled"))],
    );
    across_a_gap(&conn, "tracked", 2_620);
    let costs = resolve(&conn, "tracked");
    assert_eq!(costs.cycled_basis(), CostBasis::Tracked);
    assert_eq!(costs.across_gap, gap);
    assert_eq!(costs.shown_across_gap(), None);
}

/// A bucket is the session's play when its tracked figure is above zero or
/// any of its events was tracked: shots at a weapon with no price still are.
#[test]
fn a_bucket_is_played_by_a_tracked_figure_or_any_tracked_event() {
    let conn = conn();
    session(&conn, "s", 0.0, 0.4, 0.0, 0.0);
    kill(&conn, "k", "s", 1_500.0, ("Opalo", 3, 0.0), 0.0, 0.0);
    let played = session_costs::played(&conn, "s").unwrap();
    assert_eq!(
        played.into_iter().collect::<Vec<_>>(),
        [Bucket::Weapon, Bucket::Healing]
    );
    assert!(session_costs::played(&conn, "absent").unwrap().is_empty());
}

// ── A weapon whose ammo was topped up ───────────────────────────────

/// A weapon's parts and reason, when it is mixed.
fn mixed_weapon(costs: &session_costs::SessionCosts) -> (f64, f64, TrackedReason) {
    match costs.basis(Bucket::Weapon) {
        Basis::Mixed {
            measured,
            tracked,
            reason,
        } => (measured, tracked, reason),
        other => panic!("not mixed: {other:?}"),
    }
}

/// A step the session's ammo was topped up in, `(from, to]` in seconds, as
/// the account side stores it.
fn top_up(conn: &Connection, id: &str, from: f64, to: f64) {
    conn.execute(
        "INSERT INTO account_session_cost_top_ups (session_id, from_ms, to_ms) \
         VALUES (?1, ?2, ?3)",
        rusqlite::params![id, (from * 1000.0) as i64, (to * 1000.0) as i64],
    )
    .unwrap();
}

#[test]
fn a_topped_up_weapon_is_its_measured_part_and_the_tracked_kills_of_the_top_up_step() {
    let conn = conn();
    // Tracked: the rifle's kill (1.00) at 1500, the pistol's (1.00) at
    // 1600, and 0.50 after the last kill. The readings measured 2.00 of
    // everything but the ammo fired in the step the ammo arrived in, which
    // ended with the rifle's kill.
    hunted(&conn, "s1");
    store(&conn, "s1", CURRENT, &[("weapon", Stored::Mixed(2.0))]);
    top_up(&conn, "s1", 1450.0, 1500.0);
    let costs = resolve(&conn, "s1");
    let (measured, tracked, reason) = mixed_weapon(&costs);
    assert_near(measured, 2.0);
    assert_near(tracked, 1.0);
    assert_eq!(reason, TrackedReason::AmmoToppedUp);
    assert_eq!(costs.cost_basis(Bucket::Weapon), CostBasis::Mixed);
    // The 3.00 is shared over the kills and the dangling spend as a
    // measured weapon is.
    assert_near(costs.scale(Bucket::Weapon), 1.2);
    assert_near(costs.resolved.weapon, 2.4);
    assert_near(costs.resolved.dangling, 0.6);
    // Every SQL reader weighs the kills by the same scale.
    let scale: f64 = conn
        .query_row(
            &format!(
                "SELECT {}",
                session_costs::scale_sql(Bucket::Weapon, "'s1'")
            ),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_near(scale, 1.2);
    // The detail shows the whole weapon, mixed, and why.
    let detail = crate::tracking_reads::get_session_read(&conn, "s1", now())
        .unwrap()
        .unwrap();
    let breakdown = &detail["summary"]["costBreakdown"];
    assert_near(breakdown["weaponCost"].as_f64().unwrap(), 3.0);
    assert_eq!(breakdown["basis"]["weaponCost"], "mixed");
    assert_eq!(breakdown["reasons"]["weaponCost"], "ammoToppedUp");

    // A kill ending as the step opens was fought before it: not in it.
    session_costs::forget(&conn, "s1").unwrap();
    conn.execute("DELETE FROM account_session_cost_top_ups", [])
        .unwrap();
    top_up(&conn, "s1", 1500.0, 1550.0);
    let costs = resolve(&conn, "s1");
    let (measured, tracked, reason) = mixed_weapon(&costs);
    assert_near(measured, 2.0);
    assert_near(tracked, 0.0);
    assert_eq!(reason, TrackedReason::AmmoToppedUp);
}

#[test]
fn a_top_up_step_past_the_last_kill_with_shots_after_it_keeps_the_weapon_tracked() {
    let conn = conn();
    hunted(&conn, "s1");
    store(&conn, "s1", CURRENT, &[("weapon", Stored::Mixed(2.0))]);
    // The step runs past the pistol's kill at 1600, and 0.50 of shots
    // after it carry no time: how much of them it holds is not known.
    top_up(&conn, "s1", 1550.0, 1650.0);
    let costs = resolve(&conn, "s1");
    assert_eq!(
        costs.basis(Bucket::Weapon),
        Basis::Tracked(TrackedReason::AmmoToppedUp)
    );
    assert_eq!(costs.resolved.weapon, costs.tracked.weapon);
    assert_eq!(costs.resolved.dangling, costs.tracked.dangling);
    assert_eq!(costs.scale(Bucket::Weapon), 1.0);
}

#[test]
fn a_top_up_step_past_the_last_kill_with_nothing_fired_after_it_stays_mixed() {
    let conn = conn();
    session(&conn, "s1", 0.0, 0.0, 0.0, 0.0);
    kill(&conn, "k1", "s1", 1500.0, ("Rifle", 20, 0.05), 0.0, 1.0);
    store(&conn, "s1", CURRENT, &[("weapon", Stored::Mixed(1.5))]);
    top_up(&conn, "s1", 1450.0, 1700.0);
    let costs = resolve(&conn, "s1");
    let (measured, tracked, reason) = mixed_weapon(&costs);
    assert_near(measured, 1.5);
    assert_near(tracked, 1.0);
    assert_eq!(reason, TrackedReason::AmmoToppedUp);
    assert_near(costs.resolved.weapon, 2.5);
}

#[test]
fn a_mixed_weapon_s_daily_rollup_is_its_measured_part_and_its_tracked_part() {
    let conn = conn();
    hunted(&conn, "s1");
    store(&conn, "s1", CURRENT, &[("weapon", Stored::Mixed(2.0))]);
    top_up(&conn, "s1", 1450.0, 1500.0);
    session_costs::refresh(&conn, "s1").unwrap();
    crate::daily_rollup::recompute_day(&conn, "1970-01-01").unwrap();
    let weapon: f64 = conn
        .query_row(
            "SELECT COALESCE(weapon_cost, 0) + COALESCE(dangling_cost, 0) FROM daily_rollups \
             WHERE day = '1970-01-01'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // 2.00 measured and the rifle's 1.00 tracked kill in the top-up step.
    assert_near(weapon, 3.0);
}

#[tokio::test]
async fn a_mixed_weapon_no_tracked_shot_carries_sits_at_the_session_grain_in_every_reader() {
    let (_dir, db) = open().await;
    db.with_writer(|conn| {
        // Nothing tracked: no kill, no shot after one.
        session(conn, "s1", 0.0, 0.0, 0.0, 0.0);
        store(conn, "s1", CURRENT, &[("weapon", Stored::Mixed(1.5))]);
        top_up(conn, "s1", 1000.0, 1060.0);
        Ok(())
    })
    .await
    .unwrap();
    // The summaries' heal owes it a summary on its mixed weapon alone.
    let page = crate::tracking_reads::list_sessions_impl(&db, now(), None, None, None)
        .await
        .unwrap();
    assert_near(page.sessions[0]["cost"].as_f64().unwrap(), 1.5);
    let (costs, total, weapon_grain, spent) = db
        .with_reader(|conn| {
            let costs = session_costs::resolve(conn, "s1")?;
            let total: f64 = conn.query_row(
                &format!(
                    "SELECT {} FROM tracking_sessions s WHERE s.id = 's1'",
                    session_costs::session_total_sql("s")
                ),
                [],
                |row| row.get(0),
            )?;
            let weapon_grain: Option<f64> = conn.query_row(
                &format!(
                    "SELECT {} FROM tracking_sessions s WHERE s.id = 's1'",
                    session_costs::session_sums_sql("s")
                ),
                [],
                |row| row.get(4),
            )?;
            let spent: bool = conn.query_row(
                &format!(
                    "SELECT {} FROM tracking_sessions s WHERE s.id = 's1'",
                    session_costs::spent_anything_sql("s")
                ),
                [],
                |row| row.get(0),
            )?;
            Ok((costs, total, weapon_grain, spent))
        })
        .await
        .unwrap();
    let (measured, tracked, _) = mixed_weapon(&costs);
    assert_near(measured + tracked, 1.5);
    assert_eq!(costs.scale(Bucket::Weapon), 1.0, "no tracked shot to scale");
    assert_near(costs.resolved.weapon, 1.5);
    // The aggregate readers sum the same: measured plus tracked, whole at
    // the session's grain.
    assert_near(total, measured + tracked);
    assert_near(
        weapon_grain.expect("a session-grain weapon"),
        measured + tracked,
    );
    // The mixed weapon alone is what the session spent.
    assert!(spent);
    let report = crate::maintenance::rebuild_and_verify(&db, now())
        .await
        .unwrap();
    assert!(report.all_matched(), "{:?}", report.tables);
}
