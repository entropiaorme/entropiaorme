//! Behavioural pin for a quest reward's arrival over the typed facade.
//!
//! The invariant: a reward clump a hand-in claimed is recorded as a row of
//! `kills` (it arrives as a mob-less loot clump), but it is no kill. Every
//! cross-session statistic and the session's own list row and detail read
//! as if the row had never been recorded. Two worlds over the same base
//! history are compared:
//!
//! * `never`: the base history alone;
//! * `claimed`: the base plus the arrival, claimed (its loot lines
//!   deactivated, its loot total zeroed, `reward_claimed_at` set).
//!
//! A third world holds the same row unclaimed, the fixture's proof that the
//! arrival moves the compared kill figures at all.

use std::collections::BTreeMap;

use eo_api::Api;
use eo_services::db::Db;
use serde_json::Value;

use crate::session_exclusion_facade::{
    aggregates, api_over, assert_projections_current, assert_same, run, seed_base, DAY,
};

/// The reward lands in session `a`, after its two kills, as the latest row.
async fn seed_arrival(db: &Db, claimed: bool) {
    let at = DAY + 3_600.0 + 600.0;
    let (claimed_at, deactivated_at) = if claimed {
        (format!("{at}"), format!("{at}"))
    } else {
        ("NULL".to_string(), "NULL".to_string())
    };
    run(
        db,
        format!(
            "INSERT INTO kills (id, loot_source_id, session_id, mob_name, mob_species, \
               mob_maturity, timestamp, shots_fired, damage_dealt, cost_ped, enhancer_cost, \
               loot_total_ped, context_id, reward_claimed_at) \
             VALUES ('a-reward', 'clump-a', 'a', 'Unknown', '', '', {at}, 0, 0, 0, 0, 0, 1, \
               {claimed_at});
             INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, deactivated_at) \
             VALUES ('a-reward', 'Blazar Fragment', 100, 0.001, {deactivated_at});"
        ),
    )
    .await;
}

/// The cross-session statistics plus the session's own list row and detail
/// (the list row carries no kill count today; it is compared so one that
/// gains a kill figure is covered from the start).
async fn reads(api: &Api) -> BTreeMap<&'static str, Value> {
    let mut out = aggregates(api).await;
    let detail = api.tracking_session_detail("a".to_string()).await.unwrap();
    out.insert(
        "tracking_session_detail",
        serde_json::to_value(detail).unwrap(),
    );
    let page = api.tracking_sessions(None, None, None).await.unwrap();
    out.insert("tracking_sessions", serde_json::to_value(page).unwrap());
    out
}

async fn world(claimed: Option<bool>) -> [BTreeMap<&'static str, Value>; 2] {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_base(&db).await;
    if let Some(claimed) = claimed {
        seed_arrival(&db, claimed).await;
    }
    let first = reads(&api).await;
    assert_projections_current(&db).await;
    let second = reads(&api).await;
    [first, second]
}

#[tokio::test]
async fn a_claimed_reward_arrival_reads_as_never_a_kill() {
    let never = world(None).await;
    let unclaimed = world(Some(false)).await;
    // The unclaimed arrival is a zero-loot kill, so only the kill figures can
    // tell it apart: these reads carry them.
    for name in [
        "analytics_hunting",
        "tracking_snapshot",
        "tracking_session_detail",
    ] {
        assert_ne!(
            unclaimed[1].get(name),
            never[1].get(name),
            "an unclaimed arrival does not reach {name}, so its comparison proves nothing"
        );
    }

    let claimed = world(Some(true)).await;
    assert_same("claimed", &claimed[0], &never[0]);
    assert_same("claimed, rebuilt", &claimed[1], &never[1]);
}
