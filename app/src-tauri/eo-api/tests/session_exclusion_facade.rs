//! Behavioural pins for session stats exclusion over the typed facade.
//!
//! The invariant: a session excluded from a stats scope is
//! indistinguishable, in every cross-session figure of that scope, from a
//! session that was never recorded. Each test builds worlds over the same
//! base history and compares every aggregate command between them:
//!
//! * `never`: the base history alone;
//! * `excluded`: the base plus two extra sessions (one rolled into the
//!   daily projections, one from today, served raw), both excluded;
//! * `included`: the base plus the extras, counted (the fixture's proof
//!   that the extras move every compared figure at all).
//!
//! Seeded figures are exact binary fractions, so sums agree bit for bit
//! whatever order a reader folds them in.
//!
//! The classification guard at the end makes every command on the typed
//! surface declare whether it serves cross-session statistics, so a new
//! aggregate cannot ship without joining this comparison.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use eo_api::loadout_comparison::{ComponentPick, SetupPick};
use eo_api::skilling::{SkillingForecastQuery, SkillingRoutePick, SkillingTargetKind};
use eo_api::Api;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use serde_json::{json, Value};

mod common;

/// 2026-01-10 00:00 UTC: the base history's first day, long rolled up.
const DAY: f64 = 1_768_003_200.0;

/// The session definition every seeded session runs under (the protected
/// default holds the first id).
const DEFINITION: i64 = 7;

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        .floor()
}

/// A catalogue with the sword the sessions fire, the mob they hunt, and
/// the professions a skilling forecast prices.
fn write_snapshot(dir: &Path) {
    let write = |name: &str, value: Value| {
        std::fs::write(dir.join(name), value.to_string()).unwrap();
    };
    write(
        "weapons.json",
        json!([
            {"id": "sword", "name": "Laconian Sword",
             "economy": {"decay": 1.95, "ammo_burn": 0, "efficiency": 54.9, "max_tt": 8},
             "damage": {"impact": 1, "cut": 3, "stab": 3, "penetration": 1}, "uses_per_minute": 67,
             "professions": {"hit": "Swordsman (Hit)", "dmg": "Swordsman (Dmg)"}}
        ]),
    );
    write("weapon_amplifiers.json", json!([]));
    write("absorbers.json", json!([]));
    write("mindforce_implants.json", json!([]));
    write("medical_tools.json", json!([]));
    write(
        "mobs.json",
        json!([
            {"name": "Carabok", "species": {"name": "Carabok", "codex_base_cost": 1.25, "codex_type": "Mob"},
             "planet": "Arkadia", "maturities": [{"name": "Puny", "hp": 10, "level": 4}]}
        ]),
    );
    let profession = |name: &str, skills: &[(&str, f64)]| {
        json!({"name": name, "skills": skills.iter()
            .map(|(skill, weight)| json!({"skill": {"name": skill}, "weight": weight}))
            .collect::<Vec<_>>()})
    };
    write(
        "professions.json",
        json!([
            profession(
                "Swordsman (Hit)",
                &[("Longblades", 60.0), ("Anatomy", 40.0)]
            ),
            profession(
                "Swordsman (Dmg)",
                &[("Longblades", 50.0), ("Anatomy", 50.0)]
            ),
            profession("Animal Looter", &[("Looting", 100.0)]),
            profession("Mutant Looter", &[("Looting", 100.0)]),
            profession("Robot Looter", &[("Looting", 100.0)]),
        ]),
    );
    write(
        "skills.json",
        json!([{"name": "Longblades"}, {"name": "Anatomy"}, {"name": "Looting"}]),
    );
}

async fn api_over(dir: &Path) -> (Api, Db) {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    write_snapshot(&snapshot);
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("snapshot store"));
    let clock = Arc::new(eo_services::clock::RealClock::new());
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    (
        Api::new(
            db.clone(),
            game_data,
            clock,
            data_dir,
            handles.config_service,
            handles.tracker,
            handles.hotbar,
            handles.watcher,
            handles.skill_tracker,
            handles.skill_scan,
            handles.spacebar,
            handles.repair_ocr,
            handles.sale_window_ocr,
            handles.quests.clone(),
            None,
            None,
            None,
        ),
        db,
    )
}

async fn run(db: &Db, sql: String) {
    db.with_writer(move |conn| {
        conn.execute_batch(&sql)?;
        Ok(())
    })
    .await
    .unwrap();
}

/// One ended hunting session of the definition, with its context, kills
/// (each with its sword phase and loot), and skill gains.
struct SessionSeed {
    id: &'static str,
    context: i64,
    started: f64,
    ended: f64,
    /// (armour, heal, dangling, consumable) session costs.
    costs: (f64, f64, f64, f64),
    /// (kill id, offset from start, cost, enhancer cost, loot) per kill.
    kills: &'static [(&'static str, f64, f64, f64, f64)],
    /// (skill, PES) gains, half-way through.
    gains: &'static [(&'static str, f64)],
    /// (harvest id, cost, loot, tier) swings, half-way through.
    harvests: &'static [(i64, f64, f64, &'static str)],
}

fn session_sql(seed: &SessionSeed) -> String {
    let SessionSeed {
        id,
        context,
        started,
        ended,
        costs: (armour, heal, dangling, consumable),
        ..
    } = seed;
    let mut sql = format!(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, armour_cost, \
           heal_cost, dangling_cost, consumable_cost, mob_tracking_mode, session_name, \
           definition_id, skill_boost_percent) \
         VALUES ('{id}', {started}, {ended}, 0, {armour}, {heal}, {dangling}, {consumable}, \
           'mob', 'Carabok run', 7, 50);
         INSERT INTO session_contexts (id, session_id, created_at) VALUES ({context}, '{id}', {started});"
    );
    for (kill, offset, cost, enhancer, loot) in seed.kills {
        let at = started + offset;
        let shots = cost / 0.25;
        sql.push_str(&format!(
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
               shots_fired, damage_dealt, cost_ped, enhancer_cost, loot_total_ped, context_id) \
             VALUES ('{kill}', '{id}', 'Carabok Puny', 'Carabok', 'Puny', {at}, {shots}, 10, \
               {cost}, {enhancer}, {loot}, {context});
             INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
               cost_per_shot) VALUES ('{kill}', 'Laconian Sword', {shots}, 10, 0.25);
             INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped) \
               VALUES ('{kill}', 'Animal Oil Residue', 10, {loot});"
        ));
    }
    let middle = (started + ended) / 2.0;
    for (skill, pes) in seed.gains {
        sql.push_str(&format!(
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value, \
               context_id) VALUES ('{id}', {middle}, '{skill}', 0.5, {pes}, {context});"
        ));
    }
    for (harvest, cost, loot, tier) in seed.harvests {
        sql.push_str(&format!(
            "INSERT INTO harvest_events (id, session_id, timestamp, success, tool_name, cost_ped, \
               loot_total_ped, yield_tier, context_id) \
             VALUES ({harvest}, '{id}', {middle}, 1, 'Hatchet', {cost}, {loot}, '{tier}', {context});
             INSERT INTO harvest_loot_items (harvest_id, item_name, quantity, value_ped) \
               VALUES ({harvest}, 'Wood Shavings', 4, {loot});"
        ));
    }
    sql
}

/// The definition, the scanned levels, and two ended sessions on
/// consecutive days with a manual ledger gain between them.
async fn seed_base(db: &Db) {
    let mut sql = String::from(
        "INSERT INTO session_definitions (id, name) VALUES (7, 'Carabok run');
         INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES
           ('Longblades', 2000.0, 'scan', 1700000000.0),
           ('Anatomy', 3000.0, 'scan', 1700000000.0),
           ('Looting', 4000.0, 'scan', 1700000000.0);
         INSERT INTO ledger_entries (id, date, type, description, amount, tag)
           VALUES ('manual', '2026-01-11', 'markup', 'Sold oil', 5.0, 'misc');",
    );
    sql.push_str(&session_sql(&SessionSeed {
        id: "a",
        context: 1,
        started: DAY + 3_600.0,
        ended: DAY + 7_200.0,
        costs: (1.5, 0.5, 0.0, 0.0),
        kills: &[("a1", 400.0, 2.5, 0.25, 3.0), ("a2", 500.0, 2.5, 0.0, 1.5)],
        gains: &[("Longblades", 0.75), ("Anatomy", 0.25)],
        harvests: &[],
    }));
    sql.push_str(&session_sql(&SessionSeed {
        id: "b",
        context: 2,
        started: DAY + 86_400.0 + 3_600.0,
        ended: DAY + 86_400.0 + 7_200.0,
        costs: (0.0, 0.25, 0.0, 0.0),
        kills: &[("b1", 300.0, 5.0, 0.0, 4.0)],
        gains: &[("Longblades", 0.5)],
        harvests: &[(1, 0.5, 2.0, "short")],
    }));
    run(db, sql).await;
}

/// Two extra sessions with large, distinctive figures in every family:
/// `x` on the base's first day (rolled up), `y` from the last hours
/// (served raw), each with a derived enhancer rebate in the ledger.
async fn seed_extras(db: &Db) {
    let today = now();
    let mut sql = session_sql(&SessionSeed {
        id: "x",
        context: 3,
        started: DAY + 10_000.0,
        ended: DAY + 14_000.0,
        costs: (7.5, 2.5, 0.25, 0.5),
        kills: &[
            ("x1", 100.0, 12.5, 1.25, 50.0),
            ("x2", 900.0, 12.5, 0.0, 25.0),
        ],
        gains: &[("Longblades", 8.0), ("Anatomy", 2.0)],
        harvests: &[(2, 3.0, 9.0, "huge")],
    });
    sql.push_str(&session_sql(&SessionSeed {
        id: "y",
        context: 4,
        started: today - 7_200.0,
        ended: today - 3_600.0,
        costs: (0.0, 1.25, 0.0, 0.0),
        kills: &[("y1", 1_800.0, 6.25, 0.0, 20.0)],
        gains: &[("Longblades", 4.0)],
        harvests: &[(3, 1.0, 6.0, "long")],
    }));
    sql.push_str(
        "INSERT INTO ledger_entries (id, date, type, description, amount, tag, source_session_id)
           VALUES ('rebate-x', '2026-01-10T03:40:00.000000', 'markup',
                   'Enhancer Shrapnel Rebate', 1.0, 'enhancer', 'x');",
    );
    run(db, sql).await;
}

const EXTRAS: [&str; 2] = ["x", "y"];

/// The commands that serve cross-session statistics, each read once.
/// Keep in step with [`STATS_COMMANDS`].
async fn aggregates(api: &Api) -> BTreeMap<&'static str, Value> {
    let mut out = BTreeMap::new();
    let value = |result: Result<Value, eo_api::ApiError>, name: &str| {
        result.unwrap_or_else(|error| panic!("{name} failed: {error:?}"))
    };
    macro_rules! read {
        ($name:literal, $call:expr) => {
            out.insert(
                $name,
                value($call.await.map(|r| serde_json::to_value(r).unwrap()), $name),
            );
        };
    }
    read!("analytics_overview", api.analytics_overview("all"));
    read!("analytics_hunting", api.analytics_hunting());
    read!("analytics_harvest", api.analytics_harvest("all"));
    read!(
        "analytics_hunting_activity",
        api.analytics_hunting_activity("all")
    );
    read!("ledger_summary", api.ledger_summary("all"));
    read!("hunting_realised_markup", api.hunting_realised_markup());
    read!("harvest_realised_markup", api.harvest_realised_markup());
    read!(
        "character_skilling_forecast",
        api.character_skilling_forecast(&forecast_query())
    );
    // The snapshot carries live fields (clock, listener state); the
    // statistic in it is the selected definition's lifetime aggregate.
    api.tracking_definition_select(Some(DEFINITION))
        .await
        .unwrap();
    let snapshot = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    out.insert("tracking_snapshot", snapshot["lifetime"].clone());
    out
}

fn forecast_query() -> SkillingForecastQuery {
    SkillingForecastQuery {
        target: SkillingTargetKind::Profession,
        professions: vec!["Swordsman (Hit)".into()],
        goal: 5.0,
        routes: vec![SkillingRoutePick {
            setup: SetupPick {
                weapon: ComponentPick {
                    catalog_id: "sword".to_string(),
                    markup_percent: None,
                },
                amplifier: None,
                absorber: None,
                implant: None,
            },
            species: "Carabok".to_string(),
            maturity: "Puny".to_string(),
            loot_markup_percent: None,
            hunting_share_percent: None,
        }],
        skill_boost_percent: Some(50),
        looter_level: None,
        session_weapon: None,
    }
}

/// Every projection a rebuild produces must match what the incremental
/// maintenance left, exclusion toggles included.
async fn assert_projections_current(db: &Db) {
    let report = eo_services::maintenance::rebuild_and_verify(db, now())
        .await
        .unwrap();
    for table in &report.tables {
        assert!(
            table.matched,
            "{} drifted from a rebuild after the exclusion",
            table.table
        );
    }
}

fn assert_same(
    label: &str,
    left: &BTreeMap<&'static str, Value>,
    right: &BTreeMap<&'static str, Value>,
) {
    for (name, value) in left {
        assert_eq!(
            Some(value),
            right.get(name),
            "{label}: {name} differs from the never-recorded history"
        );
    }
}

async fn exclude(api: &Api, economy: bool, progression: bool) {
    for id in EXTRAS {
        api.tracking_session_exclusion(id.to_string(), economy, progression, None)
            .await
            .unwrap();
    }
}

/// The comparison every world is read at: once as the reads left the
/// projections, then again after a full rebuild has settled every one.
async fn both_settlements(api: &Api, db: &Db) -> [BTreeMap<&'static str, Value>; 2] {
    let first = aggregates(api).await;
    assert_projections_current(db).await;
    let second = aggregates(api).await;
    [first, second]
}

#[tokio::test]
async fn an_excluded_session_reads_as_never_recorded() {
    let never_dir = tempfile::tempdir().unwrap();
    let (never, never_db) = api_over(never_dir.path()).await;
    seed_base(&never_db).await;
    let never = both_settlements(&never, &never_db).await;

    let included_dir = tempfile::tempdir().unwrap();
    let (included, included_db) = api_over(included_dir.path()).await;
    seed_base(&included_db).await;
    seed_extras(&included_db).await;
    let included = both_settlements(&included, &included_db).await;
    // The fixture moves every figure the comparison covers (sales are
    // pinned by their own test below).
    for (name, value) in &included[1] {
        if name.ends_with("realised_markup") {
            continue;
        }
        assert_ne!(
            Some(value),
            never[1].get(name),
            "the extra sessions do not reach {name}, so its comparison proves nothing"
        );
    }

    let excluded_dir = tempfile::tempdir().unwrap();
    let (excluded, excluded_db) = api_over(excluded_dir.path()).await;
    seed_base(&excluded_db).await;
    seed_extras(&excluded_db).await;
    // Read first, so the daily projections already hold the extras and the
    // toggle has to repair them.
    aggregates(&excluded).await;
    exclude(&excluded, true, true).await;
    let excluded = both_settlements(&excluded, &excluded_db).await;

    assert_same("excluded", &excluded[0], &never[0]);
    assert_same("excluded, rebuilt", &excluded[1], &never[1]);
}

#[tokio::test]
async fn including_a_session_again_restores_every_figure() {
    let included_dir = tempfile::tempdir().unwrap();
    let (included, included_db) = api_over(included_dir.path()).await;
    seed_base(&included_db).await;
    seed_extras(&included_db).await;
    let included = both_settlements(&included, &included_db).await;

    let restored_dir = tempfile::tempdir().unwrap();
    let (restored, restored_db) = api_over(restored_dir.path()).await;
    seed_base(&restored_db).await;
    seed_extras(&restored_db).await;
    aggregates(&restored).await;
    exclude(&restored, true, true).await;
    aggregates(&restored).await;
    exclude(&restored, false, false).await;
    let restored = both_settlements(&restored, &restored_db).await;

    assert_eq!(restored[0], included[0]);
    assert_eq!(restored[1], included[1]);
}

#[tokio::test]
async fn a_deleted_session_reads_as_never_recorded() {
    let never_dir = tempfile::tempdir().unwrap();
    let (never, never_db) = api_over(never_dir.path()).await;
    seed_base(&never_db).await;
    let never = both_settlements(&never, &never_db).await;

    let deleted_dir = tempfile::tempdir().unwrap();
    let (deleted, deleted_db) = api_over(deleted_dir.path()).await;
    seed_base(&deleted_db).await;
    seed_extras(&deleted_db).await;
    aggregates(&deleted).await;
    for id in EXTRAS {
        deleted
            .tracking_session_delete(id.to_string())
            .await
            .unwrap();
    }
    let deleted = both_settlements(&deleted, &deleted_db).await;

    assert_same("deleted", &deleted[0], &never[0]);
    assert_same("deleted, rebuilt", &deleted[1], &never[1]);
}

/// Excluding one scope moves that scope's figures and leaves the other's.
#[tokio::test]
async fn each_scope_is_excluded_on_its_own() {
    let never_dir = tempfile::tempdir().unwrap();
    let (never, never_db) = api_over(never_dir.path()).await;
    seed_base(&never_db).await;
    let never = aggregates(&never).await;

    let included_dir = tempfile::tempdir().unwrap();
    let (included, included_db) = api_over(included_dir.path()).await;
    seed_base(&included_db).await;
    seed_extras(&included_db).await;
    let included = aggregates(&included).await;

    // The economy alone: every PED family reads as never recorded, the
    // skill gains still count.
    let economy_dir = tempfile::tempdir().unwrap();
    let (economy, economy_db) = api_over(economy_dir.path()).await;
    seed_base(&economy_db).await;
    seed_extras(&economy_db).await;
    exclude(&economy, true, false).await;
    assert_projections_current(&economy_db).await;
    let economy = aggregates(&economy).await;
    let overview = |world: &BTreeMap<&'static str, Value>| world["analytics_overview"].clone();
    assert_eq!(
        overview(&economy)["returnsBreakdown"]["pes"],
        overview(&included)["returnsBreakdown"]["pes"]
    );
    assert_eq!(
        overview(&economy)["returnsBreakdown"]["lootTt"],
        overview(&never)["returnsBreakdown"]["lootTt"]
    );
    assert_eq!(
        overview(&economy)["lossesBreakdown"],
        overview(&never)["lossesBreakdown"]
    );
    assert_eq!(
        overview(&economy)["returnsBreakdown"]["ledger"],
        overview(&never)["returnsBreakdown"]["ledger"]
    );
    for name in [
        "analytics_hunting",
        "analytics_harvest",
        "analytics_hunting_activity",
        "ledger_summary",
    ] {
        assert_eq!(
            economy[name], never[name],
            "{name} still counts the economy-excluded sessions"
        );
    }
    // The family's lifetime keeps the PES and drops the rest.
    let lifetime = &economy["tracking_snapshot"];
    assert_eq!(lifetime["pes"], included["tracking_snapshot"]["pes"]);
    assert_eq!(lifetime["cycled"], never["tracking_snapshot"]["cycled"]);
    assert_eq!(
        lifetime["instanceCount"],
        never["tracking_snapshot"]["instanceCount"]
    );
    // A forecast session spans both scopes, so it leaves whole.
    assert_eq!(
        economy["character_skilling_forecast"],
        never["character_skilling_forecast"]
    );

    // The skill gains alone: the PES reads as never recorded, the PED side
    // still counts, and a PES rate pairs only the sessions whose gains count.
    let progression_dir = tempfile::tempdir().unwrap();
    let (progression, progression_db) = api_over(progression_dir.path()).await;
    seed_base(&progression_db).await;
    seed_extras(&progression_db).await;
    exclude(&progression, false, true).await;
    assert_projections_current(&progression_db).await;
    let progression = aggregates(&progression).await;
    assert_eq!(
        overview(&progression)["returnsBreakdown"]["pes"],
        overview(&never)["returnsBreakdown"]["pes"]
    );
    assert_eq!(
        overview(&progression)["lossesBreakdown"],
        overview(&included)["lossesBreakdown"]
    );
    let rows = |world: &BTreeMap<&'static str, Value>| {
        world["analytics_hunting"]["nameComparisons"][0].clone()
    };
    assert_eq!(rows(&progression)["cycled"], rows(&included)["cycled"]);
    assert_eq!(
        rows(&progression)["pesPer100Ped"],
        rows(&never)["pesPer100Ped"]
    );
    assert_ne!(
        rows(&progression)["pesPer100Ped"],
        rows(&included)["pesPer100Ped"]
    );
    assert_eq!(
        progression["analytics_hunting_activity"],
        included["analytics_hunting_activity"]
    );
    let lifetime = &progression["tracking_snapshot"];
    assert_eq!(lifetime["pes"], never["tracking_snapshot"]["pes"]);
    assert_eq!(lifetime["cycled"], included["tracking_snapshot"]["cycled"]);
    // The recorded sources span both scopes and leave; a planned route's
    // pace and the session weapons are economy evidence and stay.
    let forecast =
        |world: &BTreeMap<&'static str, Value>| world["character_skilling_forecast"].clone();
    assert_eq!(
        forecast(&progression)["sources"],
        forecast(&never)["sources"]
    );
    assert_ne!(
        forecast(&progression)["sources"],
        forecast(&included)["sources"]
    );
    assert_eq!(
        forecast(&progression)["routes"],
        forecast(&included)["routes"]
    );
    assert_eq!(
        forecast(&progression)["sessionWeapons"],
        forecast(&included)["sessionWeapons"]
    );
}

/// A confirmed sale drawing on an excluded session's loot keeps its share
/// in the denominator but credits it to no activity, exactly as stock sold
/// beyond tracking stays unclaimed; the other share is unchanged.
#[tokio::test]
async fn a_sale_share_from_an_excluded_session_stays_unclaimed() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_base(&db).await;
    seed_extras(&db).await;
    // One sold listing: 10 units of oil, half from session `a` (context 1),
    // half from `x` (context 3), 2.5 PED of markup over TT.
    run(
        &db,
        "INSERT INTO auction_listings (id, item_name, profession, quantity, attributed_qty, \
           tt_value, attributed_tt, starting_bid, listing_fee, listed_at, status, final_price, \
           sale_fee, created_at, updated_at, subject_kind) \
         VALUES ('sale', 'Animal Oil Residue', 'hunting', 10, 10, 5.0, 5.0, 7.5, 0, \
           '2026-01-12', 'sold', 7.5, 0, 0, 0, 'loot');
         INSERT INTO stock_movements (item_name, movement_kind, ref_id, source_kind, mob_species, \
           quantity, tt_value, occurred_at, created_at, session_definition_id, activity_context_id) \
         VALUES ('Animal Oil Residue', 'listing', 'sale', 'hunt', 'Carabok', -5, 2.5, \
           '2026-01-12', 0, 1, 1), \
           ('Animal Oil Residue', 'listing', 'sale', 'hunt', 'Carabok', -5, 2.5, \
           '2026-01-12', 0, 1, 3);"
            .to_string(),
    )
    .await;
    let markup = |value: Value| -> (f64, f64) {
        (
            value["species"][0]["netMarkup"].as_f64().unwrap_or(0.0),
            value["definitions"][0]["netMarkup"].as_f64().unwrap_or(0.0),
        )
    };
    let counted =
        markup(serde_json::to_value(api.hunting_realised_markup().await.unwrap()).unwrap());
    api.tracking_session_exclusion("x".to_string(), true, false, None)
        .await
        .unwrap();
    let excluded =
        markup(serde_json::to_value(api.hunting_realised_markup().await.unwrap()).unwrap());
    assert!(
        counted.0 > 0.0 && counted.1 > 0.0,
        "the sale credits markup: {counted:?}"
    );
    assert_eq!(excluded.0, counted.0 / 2.0);
    assert_eq!(excluded.1, counted.1 / 2.0);
}

/// The command contract: guards, idempotent stamps, the note, and the
/// list row and detail carrying the state.
#[tokio::test]
async fn the_exclusion_command_contract() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_base(&db).await;
    run(
        &db,
        "INSERT INTO tracking_sessions (id, started_at, is_active, mob_tracking_mode) \
         VALUES ('live', 1.0, 1, 'mob');"
            .to_string(),
    )
    .await;

    let kind = |error: eo_api::ApiError| serde_json::to_value(&error).unwrap()["kind"].clone();
    let missing = api
        .tracking_session_exclusion("nope".into(), true, false, None)
        .await
        .unwrap_err();
    assert_eq!(kind(missing), "notFound");
    let running = api
        .tracking_session_exclusion("live".into(), true, false, None)
        .await
        .unwrap_err();
    assert_eq!(kind(running), "conflict");

    let first = api
        .tracking_session_exclusion("a".into(), true, false, Some("  hotbar misread  ".into()))
        .await
        .unwrap();
    let first = serde_json::to_value(first).unwrap();
    assert_eq!(first["exclusion"]["note"], "hotbar misread");
    assert!(first["exclusion"]["economyExcludedAt"].is_string());
    assert!(first["exclusion"]["progressionExcludedAt"].is_null());

    // Repeating keeps the original stamp; adding a scope stamps only it.
    let second = api
        .tracking_session_exclusion("a".into(), true, true, Some("hotbar misread".into()))
        .await
        .unwrap();
    let second = serde_json::to_value(second).unwrap();
    assert_eq!(
        second["exclusion"]["economyExcludedAt"],
        first["exclusion"]["economyExcludedAt"]
    );
    assert!(second["exclusion"]["progressionExcludedAt"].is_string());

    // The list row and the detail carry it.
    let page =
        serde_json::to_value(api.tracking_sessions(None, None, None).await.unwrap()).unwrap();
    let row = page["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "a")
        .unwrap()
        .clone();
    assert_eq!(row["exclusion"], second["exclusion"]);
    let other = page["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "b")
        .unwrap()
        .clone();
    assert!(other["exclusion"].is_null());
    let detail =
        serde_json::to_value(api.tracking_session_detail("a".into()).await.unwrap()).unwrap();
    assert_eq!(detail["exclusion"], second["exclusion"]);
    // The session's own figures stay whole on its record.
    assert_eq!(detail["summary"]["returns"], 4.5);

    // Counting everywhere again clears the stamps and drops the note.
    let cleared = api
        .tracking_session_exclusion("a".into(), false, false, Some("kept?".into()))
        .await
        .unwrap();
    assert!(serde_json::to_value(cleared).unwrap()["exclusion"].is_null());
    let note: Option<String> = db
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT exclusion_note FROM tracking_sessions WHERE id = 'a'",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(note, None);
}

/// The commands that serve cross-session statistics: each is compared by
/// the never-recorded test above, through [`aggregates`].
const STATS_COMMANDS: &[&str] = &[
    "analytics_overview",
    "analytics_hunting",
    "analytics_harvest",
    "analytics_hunting_activity",
    "ledger_summary",
    "hunting_realised_markup",
    "harvest_realised_markup",
    "character_skilling_forecast",
    "tracking_snapshot",
];

/// Every other command, by why it is not a cross-session statistic.
const OTHER_COMMANDS: &[&str] = &[
    // Writes and one-session reads of the tracking record itself (a
    // session's own figures stay whole on its record by design).
    "tracking_sessions",
    "tracking_session_detail",
    "tracking_session_intervals",
    "tracking_mob_catalogue",
    "tracking_start",
    "tracking_stop",
    "tracking_weapon_decide",
    "tracking_release_mob",
    "tracking_manual_mob_lock",
    "tracking_session_config",
    "tracking_activity_options",
    "tracking_activity_activate",
    "tracking_activity_deactivate",
    "tracking_definition_select",
    "tracking_reassign_session",
    "tracking_rename_mob",
    "tracking_restore_mob",
    "tracking_loot_item_activate",
    "tracking_loot_item_deactivate",
    "tracking_armour_cost",
    "tracking_repair_scan",
    "tracking_session_exclusion",
    "tracking_session_delete",
    "session_definitions_list",
    "session_definition_create",
    "session_definition_update",
    "session_definition_archive",
    "session_definition_restore",
    // Per-session evidence review and its marks, and protection cost
    // bookkeeping (where a repair's cost went is a record, not a statistic;
    // the cost itself leaves through the session's armour cost).
    "healing_outputs",
    "healing_correction_tools",
    "healing_correct",
    "healing_correction_undo",
    "weapon_shots",
    "weapon_unpriced_exposure",
    "weapon_correction_weapons",
    "weapon_assign",
    "weapon_mark_effect_tick",
    "weapon_assignment_undo",
    "weapon_bulk_options",
    "weapon_assign_fitting",
    "weapon_batch_undo",
    "consumable_doses",
    "consumable_dose_start",
    "consumable_dose_end",
    "consumable_dose_remove",
    "consumable_dose_restore",
    "consumable_session_doses",
    "protection_overview",
    "protection_set_create",
    "protection_set_update",
    "protection_set_archive",
    "protection_set_restore",
    "protection_undo",
    "protection_unrecorded_sessions",
    "protection_session_status",
    "protection_recording_candidates",
    "protection_observation_confirm",
    "protection_repair_confirm",
    "protection_trade_terminal_scan",
    // Physical stock, sales, and the ledger's own rows: holdings and
    // confirmed transactions are outside both scopes by design.
    "activity_stock",
    "auction_listings",
    "auction_listing_create",
    "auction_listing_confirm",
    "auction_listing_expire",
    "stock_convert",
    "stock_private_sale",
    "stock_remove",
    "stock_shrapnel_convert",
    "activity_history",
    "auction_sale_revert",
    "auction_listing_undo",
    "stock_conversion_undo",
    "private_sale_undo",
    "stock_removal_undo",
    "ledger_list",
    "ledger_create",
    "ledger_delete",
    "ledger_presets_list",
    "ledger_preset_create",
    "ledger_preset_delete",
    "inventory_list",
    "inventory_create",
    "inventory_update",
    "inventory_delete",
    "inventory_sell",
    "inventory_sale_window_capture",
    "inventory_sale_window_take_capture",
    "inventory_draft_resolve",
    "inventory_equipment_listing_create",
    "inventory_equipment_trade",
    // Estimated market data: informational, never accounting.
    "market_paste_preview",
    "market_paste_commit",
    "market_unit_price_set",
    "market_overview",
    "market_auction_packet_threshold",
    "market_harvest_markups",
    "market_hunt_markups",
    "market_item_history",
    // Quests and codex: confirmed hand-ins and claims, outside both scopes.
    "quests_list",
    "quest_get",
    "quest_create",
    "quest_update",
    "quest_delete",
    "quest_start",
    "quest_complete",
    "quest_hand_in_begin",
    "quest_hand_in_state",
    "quest_hand_in_wait",
    "quest_hand_in_cancel",
    "quest_hand_in_confirm",
    "quest_rewards_unresolved",
    "quest_reward_review",
    "quest_cancel",
    "quest_abandon",
    "quest_catalogue",
    "quests_mobs",
    "quests_analytics",
    "quest_families_list",
    "quest_family_create",
    "quest_family_update",
    "quest_family_delete",
    "codex_species",
    "codex_species_ranks",
    "codex_recommend",
    "codex_meta_attributes",
    "codex_meta_targets",
    "codex_meta_progress",
    "codex_meta_calibrate",
    "codex_calibrate",
    "codex_claim",
    "codex_unclaim",
    "codex_meta_claim",
    "codex_mastery_options",
    "codex_mastery_claim",
    "codex_mastery_unclaim",
    // The character: levels come from calibrations, never from gains.
    "character_calibration",
    "character_stats",
    "character_skills",
    "character_professions",
    "character_path_optimizer",
    "character_hp_optimizer",
    "character_activity_recommender",
    "scan_status",
    "scan_start",
    "scan_capture",
    "scan_cancel",
    "scan_undo",
    "scan_process",
    "scan_accept",
    "scan_reject",
    "scan_pending",
    "scan_spacebar_capture",
    // Equipment, settings, maps, navigation, and developer tools.
    "equipment_catalogue",
    "equipment_library",
    "equipment_add",
    "equipment_update",
    "equipment_delete",
    "equipment_detail",
    "loadout_compare",
    "settings_get",
    "settings_overlay_position",
    "settings_set_overlay_position",
    "settings_update",
    "dev_metrics",
    "dev_crash_reporting",
    "dev_set_crash_reporting",
    "dev_compact_database",
    "dev_rebuild_projections",
    "dev_auction_fee_research_start",
    "dev_auction_fee_research_stop",
    "dev_auction_fee_research_status",
    "dev_auction_fee_research_capture",
    "dev_auction_fee_research_overlay_status",
    "planet_maps_list",
    "map_pins_list",
    "map_pins_viewport",
    "map_pin_nearby",
    "map_views_list",
    "map_view_create",
    "map_view_rename",
    "map_view_delete",
    "map_pin_create",
    "map_pin_update",
    "map_pin_delete",
    "map_pin_cooldown",
    "pin_configs_list",
    "pin_config_create",
    "pin_config_update",
    "pin_config_delete",
    "pin_config_reorder",
    "maps_calibration_start",
    "maps_calibration_cancel",
    "maps_calibration_status",
    "maps_scan_coordinates",
    "navigation_snapshot",
    "navigation_start",
    "navigation_update_position",
    "navigation_mark_visited",
    "navigation_skip",
    "navigation_resolve_harvest",
    "navigation_undo",
    "navigation_end",
    "radar_calibration_start",
    "radar_calibration_cancel",
    "radar_calibration_status",
    "radar_geometry",
];

/// Every command on the typed surface is classified exactly once, so a new
/// cross-session statistic cannot ship without joining the comparison.
#[test]
fn every_command_declares_whether_it_serves_stats() {
    let manifest: BTreeSet<&str> = eo_api::manifest::manifest()
        .into_iter()
        .map(|command| command.name)
        .collect();
    let stats: BTreeSet<&str> = STATS_COMMANDS.iter().copied().collect();
    let other: BTreeSet<&str> = OTHER_COMMANDS.iter().copied().collect();
    assert_eq!(
        stats.len(),
        STATS_COMMANDS.len(),
        "a stats command is listed twice"
    );
    assert_eq!(
        other.len(),
        OTHER_COMMANDS.len(),
        "an other command is listed twice"
    );
    let both: Vec<_> = stats.intersection(&other).collect();
    assert!(both.is_empty(), "classified both ways: {both:?}");
    let classified: BTreeSet<&str> = stats.union(&other).copied().collect();
    let unclassified: Vec<_> = manifest.difference(&classified).collect();
    assert!(
        unclassified.is_empty(),
        "classify each new command: a cross-session statistic joins STATS_COMMANDS \
         and the never-recorded comparison; anything else joins OTHER_COMMANDS. \
         Unclassified: {unclassified:?}"
    );
    let stale: Vec<_> = classified.difference(&manifest).collect();
    assert!(
        stale.is_empty(),
        "classified but no longer on the surface: {stale:?}"
    );
}
