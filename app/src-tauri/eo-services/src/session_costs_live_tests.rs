//! A running session's live costs: what the account readings measured of
//! it up to their latest reading, and the tracked cost of the play since,
//! bucket by bucket; read back from what the account side published, and
//! taken up by the tracker's live readout once the account announces a
//! change.

use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;

use crate::bus_events::BusEvent;
use crate::chatlog_time::ChatLogClock;
use crate::clock::MockClock;
use crate::db::Db;
use crate::event_bus::EventBus;
use crate::session_costs::{
    self, Bucket, BucketCosts, LiveShare, MeasuredSoFar, Overhead, MEASURED_COSTS_VERSION,
    MEASURED_MEASURE_VERSION,
};
use crate::tracker::{HuntTracker, Providers};

fn assert_near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{actual} is not {expected}"
    );
}

/// The tracked buckets of a hunt so far.
fn tracked() -> BucketCosts {
    BucketCosts {
        weapon: 12.0,
        enhancer: 1.5,
        armour: 0.5,
        heal: 0.25,
        consumable: 0.6,
        dangling: 0.4,
        harvest: 0.8,
    }
}

/// Of those, the events timed after the latest reading.
fn after() -> BucketCosts {
    BucketCosts {
        weapon: 2.5,
        enhancer: 1.0,
        armour: 0.3,
        heal: 0.1,
        consumable: 0.2,
        dangling: 0.0,
        harvest: 0.4,
    }
}

/// Measured so far, bucket by bucket in the order of [`Bucket::ALL`].
fn so_far(cycled: [Option<f64>; 6], overhead: Overhead) -> MeasuredSoFar {
    MeasuredSoFar {
        through: 1_120.0,
        cycled,
        overhead,
    }
}

#[test]
fn with_nothing_measured_so_far_every_live_figure_is_the_tracked_one() {
    let live = session_costs::live(None, &tracked(), &after());
    assert_eq!(live.cycled, tracked());
    assert_eq!(live.overhead, Overhead::default());
    assert_eq!(live.measured_through, None);
    for bucket in Bucket::ALL {
        assert!(!live.is_measured(bucket), "{bucket:?}");
        assert_eq!(live.share(bucket), LiveShare::Tracked, "{bucket:?}");
    }
    assert_near(live.costs(), tracked().cycled());
}

#[test]
fn a_measured_bucket_is_its_measure_so_far_and_the_tracked_events_after_it() {
    let measured = so_far(
        [Some(10.0), None, Some(1.0), Some(0.3), None, Some(0.5)],
        Overhead::default(),
    );
    let live = session_costs::live(Some(&measured), &tracked(), &after());
    // The weapon's measure counts every shot, so the dangling spend joins
    // it rather than adding to it.
    assert_near(live.cycled.weapon, 12.5);
    assert_near(live.cycled.dangling, 0.0);
    assert_near(live.cycled.harvest, 1.4);
    assert_near(live.cycled.heal, 0.4);
    assert_near(live.cycled.consumable, 0.7);
    assert_eq!(live.measured_through, Some(1_120.0));
    assert_near(
        live.cycled.cycled(),
        12.5 + tracked().enhancer + tracked().armour + 0.4 + 0.7 + 1.4,
    );
}

#[test]
fn armour_is_measured_alone_up_to_the_reading() {
    // The tracked armour is what recordings spread over the session, never
    // an event of play after the reading: the measure stands alone.
    let measured = so_far(
        [None, None, None, None, Some(1.13), None],
        Overhead::default(),
    );
    let live = session_costs::live(Some(&measured), &tracked(), &after());
    assert_near(live.cycled.armour, 1.13);
    assert_eq!(live.share(Bucket::Armour), LiveShare::Whole(1.13));
    // Every other bucket is tracked.
    assert_near(live.cycled.weapon, tracked().weapon);
    assert_near(live.cycled.dangling, tracked().dangling);
    assert_near(live.cycled.heal, tracked().heal);
}

#[test]
fn a_bucket_the_readings_could_not_settle_keeps_its_tracked_figure() {
    // The ammo's split unsettled: the weapon stays tracked whole, its
    // events after the reading not added twice; the enhancer, which the
    // readings never measure, likewise.
    let measured = so_far(
        [None, None, Some(1.0), None, None, None],
        Overhead::default(),
    );
    let live = session_costs::live(Some(&measured), &tracked(), &after());
    assert_near(live.cycled.weapon, tracked().weapon);
    assert_near(live.cycled.dangling, tracked().dangling);
    assert_near(live.cycled.enhancer, tracked().enhancer);
    assert!(!live.is_measured(Bucket::Weapon));
    assert!(!live.is_measured(Bucket::Enhancer));
    assert_eq!(live.share(Bucket::Weapon), LiveShare::Tracked);
    assert!(live.is_measured(Bucket::Harvesting));
}

#[test]
fn overhead_is_measured_so_far_and_in_costs_never_in_cycled() {
    let overhead = Overhead {
        travel: 0.11,
        field_logistics: 0.18,
        unclassified: 0.05,
    };
    let measured = so_far([None; 6], overhead);
    let live = session_costs::live(Some(&measured), &tracked(), &after());
    assert_eq!(live.overhead, overhead);
    assert_eq!(live.cycled, tracked());
    assert_near(live.costs(), tracked().cycled() + 0.34);
    assert_eq!(live.measured_through, Some(1_120.0));
}

#[test]
fn tracked_lines_carry_a_measured_figure_or_it_sits_whole() {
    let measured = so_far(
        [Some(10.0), None, None, Some(0.3), None, None],
        Overhead::default(),
    );
    let with_heals = session_costs::live(Some(&measured), &tracked(), &after());
    assert_eq!(
        with_heals.share(Bucket::Weapon),
        LiveShare::Scaled(12.5 / 12.4)
    );
    assert_eq!(
        with_heals.share(Bucket::Healing),
        LiveShare::Scaled(0.4 / 0.25)
    );
    // No tracked heal to carry it: the measured healing sits whole.
    let no_heals = BucketCosts {
        heal: 0.0,
        ..tracked()
    };
    let none_after = BucketCosts {
        heal: 0.0,
        ..after()
    };
    let without = session_costs::live(Some(&measured), &no_heals, &none_after);
    assert_eq!(without.share(Bucket::Healing), LiveShare::Whole(0.3));
}

// ── Read back from what the account side published ──────────────────

fn conn() -> Connection {
    crate::db::migrated_in_memory().unwrap()
}

fn session(conn: &Connection, id: &str, running: bool) {
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES (?1, 1000.0, ?2, ?3)",
        rusqlite::params![id, (!running).then_some(2_000.0), i64::from(running)],
    )
    .unwrap();
}

/// What the account side publishes: the weapon unsettled, the armour and
/// healing measured, travel among the overhead.
fn publish(conn: &Connection, id: &str, costs_version: i64, through: f64) {
    conn.execute(
        "INSERT OR REPLACE INTO account_live_session_costs \
             (session_id, costs_version, measure_version, reading_id, measured_through, \
              weapon_micro_ped, harvesting_micro_ped, healing_micro_ped, armour_micro_ped, \
              consumables_micro_ped, travel_micro_ped, field_logistics_micro_ped, \
              unclassified_micro_ped, computed_at) \
         VALUES (?1, ?2, ?3, 1, ?4, NULL, 0, 300000, 1130000, NULL, 110000, NULL, NULL, 1.0)",
        rusqlite::params![id, costs_version, MEASURED_MEASURE_VERSION, through],
    )
    .unwrap();
}

#[test]
fn the_resolver_reads_what_was_published_of_a_session_still_running() {
    let conn = conn();
    session(&conn, "live", true);
    assert_eq!(session_costs::measured_so_far(&conn, "live").unwrap(), None);
    publish(&conn, "live", MEASURED_COSTS_VERSION, 1_120.0);
    let so_far = session_costs::measured_so_far(&conn, "live")
        .unwrap()
        .expect("published");
    assert_eq!(so_far.through, 1_120.0);
    assert_eq!(so_far.bucket(Bucket::Weapon), None);
    assert_eq!(so_far.bucket(Bucket::Enhancer), None);
    assert_eq!(so_far.bucket(Bucket::Harvesting), Some(0.0));
    assert_eq!(so_far.bucket(Bucket::Healing), Some(0.3));
    assert_eq!(so_far.bucket(Bucket::Armour), Some(1.13));
    assert_eq!(so_far.bucket(Bucket::Consumables), None);
    assert_eq!(
        so_far.overhead,
        Overhead {
            travel: 0.11,
            field_logistics: 0.0,
            unclassified: 0.0,
        }
    );

    // Published by other rules: not read.
    publish(&conn, "live", MEASURED_COSTS_VERSION + 1, 1_180.0);
    assert_eq!(session_costs::measured_so_far(&conn, "live").unwrap(), None);

    // A session no longer running keeps what was published until its
    // sealed costs are stored (which then are its figures: see below).
    session(&conn, "ended", false);
    publish(&conn, "ended", MEASURED_COSTS_VERSION, 1_120.0);
    assert_eq!(
        session_costs::measured_so_far(&conn, "ended")
            .unwrap()
            .map(|so_far| so_far.through),
        Some(1_120.0)
    );
}

// ── Between the stop and the seal ───────────────────────────────────

/// An ended session's tracked play: a kill before the reading at 1120 and
/// one after it, shots after the last kill, a heal and a dose either side.
fn ended_hunt(conn: &Connection, id: &str) {
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, armour_cost, \
         heal_cost, consumable_cost, dangling_cost) \
         VALUES (?1, 1000.0, 1200.0, 0, 0.0, 0.05, 0.3, 0.02)",
        [id],
    )
    .unwrap();
    for (kill, at, cost) in [("early", 1_100.0, 0.30), ("late", 1_150.0, 0.20)] {
        conn.execute(
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
             cost_ped, enhancer_cost, loot_total_ped) \
             VALUES (?1, ?2, 'Atrox', 'Atrox', '', ?3, ?4, 0, 0)",
            rusqlite::params![kill, id, at, cost],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) \
             VALUES (?1, 'Rifle', 10, ?2)",
            rusqlite::params![kill, cost / 10.0],
        )
        .unwrap();
    }
    for (heal, at, cost) in [("first", 1_110.0, 0.02), ("second", 1_130.0, 0.03)] {
        conn.execute(
            "INSERT INTO healing_activations (id, session_id, tool_name, intent_at, \
             observed_at, chat_timestamp, cost_ped, profile_json, provenance) \
             VALUES (?1, ?2, 'FAP', ?3, ?3, '', ?4, '{}', 'direct')",
            rusqlite::params![heal, id, at, cost],
        )
        .unwrap();
    }
    for (dose, at, cost) in [("first", 1_105.0, 0.1), ("second", 1_140.0, 0.2)] {
        conn.execute(
            "INSERT INTO consumable_doses (id, item_name, source, session_id, started_at, \
             expires_at, cost_ped, cost_tracked) VALUES (?1, 'Stim', 'manual', ?2, ?3, ?3, ?4, 1)",
            rusqlite::params![dose, id, at, cost],
        )
        .unwrap();
    }
}

/// What the account side published while it ran, up to the reading at
/// 1120: the weapon, healing, armour and consumables measured.
fn publish_ended(conn: &Connection, id: &str, computed_at: f64) {
    conn.execute(
        "INSERT OR REPLACE INTO account_live_session_costs \
             (session_id, costs_version, measure_version, reading_id, measured_through, \
              weapon_micro_ped, harvesting_micro_ped, healing_micro_ped, armour_micro_ped, \
              consumables_micro_ped, travel_micro_ped, field_logistics_micro_ped, \
              unclassified_micro_ped, computed_at) \
         VALUES (?1, ?2, ?3, 1, 1120.0, 250000, NULL, 40000, 500000, 150000, 20000, NULL, NULL, \
                 ?4)",
        rusqlite::params![
            id,
            MEASURED_COSTS_VERSION,
            MEASURED_MEASURE_VERSION,
            computed_at
        ],
    )
    .unwrap();
}

#[test]
fn between_its_stop_and_its_seal_a_session_keeps_the_blend_and_the_seal_hands_over() {
    let conn = conn();
    ended_hunt(&conn, "hunt");
    // Nothing published: tracked, as ever.
    session_costs::refresh(&conn, "hunt").unwrap();
    let tracked = session_costs::resolve(&conn, "hunt").unwrap();
    assert_eq!(
        tracked.cost_basis(Bucket::Weapon),
        session_costs::CostBasis::Tracked
    );
    assert_near(tracked.resolved.cycled(), 0.5 + 0.02 + 0.05 + 0.3);

    publish_ended(&conn, "hunt", 7.0);
    // The published row stands for the ended session until its seal.
    assert!(session_costs::measured_so_far(&conn, "hunt")
        .unwrap()
        .is_some());
    assert!(session_costs::out_of_step(&conn)
        .unwrap()
        .contains(&"hunt".to_string()));
    session_costs::refresh(&conn, "hunt").unwrap();
    assert!(session_costs::out_of_step(&conn).unwrap().is_empty());
    let pending = session_costs::resolve(&conn, "hunt").unwrap();
    // The weapon: 0.25 measured, the later kill's 0.20 and the shots after
    // the last kill, 0.02. Healing: 0.04 and the later heal's 0.03.
    // Consumables: 0.15 and the later dose's 0.20. Armour: 0.50 alone.
    assert_near(pending.resolved.weapon + pending.resolved.dangling, 0.47);
    assert_near(pending.resolved.heal, 0.07);
    assert_near(pending.resolved.consumable, 0.35);
    assert_near(pending.resolved.armour, 0.5);
    assert_near(pending.overhead.travel, 0.02);
    assert_eq!(
        pending.cost_basis(Bucket::Weapon),
        session_costs::CostBasis::Measured
    );
    // The kills share the weapon's figure.
    assert_near(pending.scale(Bucket::Weapon), 0.47 / 0.52);

    // The seal stores the measured costs and drops the published row.
    conn.execute(
        "INSERT INTO account_session_costs (session_id, costs_version, measure_version, \
         measure_computed_at, status, reason, computed_at) \
         VALUES ('hunt', ?1, ?2, 1.0, 'measured', NULL, 9.0)",
        rusqlite::params![MEASURED_COSTS_VERSION, MEASURED_MEASURE_VERSION],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO account_session_cost_buckets \
         (session_id, bucket, basis, reason, amount_micro_ped, precision) \
         VALUES ('hunt', 'weapon', 'measured', NULL, 460000, 'exact')",
        [],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM account_live_session_costs WHERE session_id = 'hunt'",
        [],
    )
    .unwrap();
    assert_eq!(session_costs::measured_so_far(&conn, "hunt").unwrap(), None);
    session_costs::refresh(&conn, "hunt").unwrap();
    let sealed = session_costs::resolve(&conn, "hunt").unwrap();
    assert_near(sealed.resolved.weapon + sealed.resolved.dangling, 0.46);
    // Every other bucket left unmeasured by the seal reads tracked.
    assert_near(sealed.resolved.heal, 0.05);
    assert_near(sealed.resolved.armour, 0.0);
}

#[test]
fn a_published_row_with_its_seal_still_to_come_never_serves_a_session_its_costs_were_stored_for() {
    let conn = conn();
    ended_hunt(&conn, "hunt");
    publish_ended(&conn, "hunt", 7.0);
    conn.execute(
        "INSERT INTO account_session_costs (session_id, costs_version, measure_version, \
         measure_computed_at, status, reason, computed_at) \
         VALUES ('hunt', ?1, ?2, 1.0, 'measured', NULL, 9.0)",
        rusqlite::params![MEASURED_COSTS_VERSION, MEASURED_MEASURE_VERSION],
    )
    .unwrap();
    assert_eq!(session_costs::measured_so_far(&conn, "hunt").unwrap(), None);
}

// ── The tracker's live readout ──────────────────────────────────────

/// Wait, a little at a time, until the readout shows what was published.
fn until_measured(
    runtime: &tokio::runtime::Runtime,
    tracker: &HuntTracker,
) -> crate::tracking_models::ActiveSessionView {
    for _ in 0..500 {
        let active = runtime
            .block_on(tracker.snapshot())
            .unwrap()
            .active
            .expect("a running session");
        if active.costs_measured_through.is_some() {
            return active;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the readout never took up what was published");
}

#[test]
fn the_live_readout_takes_what_the_account_published_once_it_announces_a_change() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let db = runtime
        .block_on(Db::open(&dir.path().join("entropia_orme.db")))
        .unwrap();
    let bus = Arc::new(EventBus::new());
    let tracker = runtime
        .block_on(HuntTracker::new(
            bus.clone(),
            db.clone(),
            Arc::new(MockClock::new(None, 0.0)),
            ChatLogClock::host_local(),
            Providers::default(),
        ))
        .unwrap();
    let session = runtime.block_on(tracker.start_session()).unwrap();

    // Nothing published: exactly the tracked readout, with nothing said of
    // the account.
    let before = runtime
        .block_on(tracker.snapshot())
        .unwrap()
        .active
        .unwrap();
    assert_eq!(before.cost, 0.0);
    assert_eq!(before.overhead_cost, None);
    assert_eq!(before.costs_measured_through, None);

    let id = session.id.clone();
    runtime
        .block_on(db.with_writer(move |conn| {
            publish(conn, &id, MEASURED_COSTS_VERSION, 1_767_225_660.0);
            Ok(())
        }))
        .unwrap();
    // Published, but not announced: the readout keeps what it has.
    let unannounced = runtime
        .block_on(tracker.snapshot())
        .unwrap()
        .active
        .unwrap();
    assert_eq!(unannounced.costs_measured_through, None);

    use eo_wire::domain_events::{AccountUpdated, AccountUpdatedPayload, AccountUpdatedTag};
    bus.publish(&BusEvent::AccountUpdated(AccountUpdated {
        topic: AccountUpdatedTag,
        event_version: 1,
        occurred_at: "2026-01-01T00:01:00+00:00".to_string(),
        payload: AccountUpdatedPayload {},
    }));
    let active = until_measured(&runtime, &tracker);
    assert_eq!(active.costs_measured_through, Some(1_767_225_660.0));
    // The armour and healing the readings measured, nothing tracked since;
    // the travel in Net, never in Cycled.
    assert_eq!(active.cost, 1.43);
    assert_eq!(active.armour_cost, 1.13);
    assert_eq!(active.overhead_cost, Some(0.11));
    assert_eq!(active.net, -1.54);
    let armour: Vec<_> = active
        .cycled_breakdown
        .iter()
        .filter(|line| line.bucket == crate::tracking_models::CycledBucket::Armour)
        .collect();
    assert_eq!(armour.len(), 1);
    assert_eq!(armour[0].label, "From your account");
    assert_near(armour[0].cost_ped, 1.13);
    runtime.block_on(tracker.stop_session()).unwrap();
}
