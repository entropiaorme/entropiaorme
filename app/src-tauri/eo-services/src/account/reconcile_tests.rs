//! Reconciling stock against the account: a shortfall leaves the open
//! positions in proportion to their sources and touches no figure of play;
//! a surplus is stock of unknown origin whose later sale books only its
//! markup; a conversion group is one conversion; ammo, equipment, attached
//! and auction rows never enter the plan; applying twice changes nothing; a
//! line whose item moved after the observation is refused; undoing restores
//! the quantities; a paste and a reading of one inventory plan alike;
//! nothing is planned or applied while a session is tracked; an item the
//! player took back from a record the readings made is left as it is until
//! a reconciliation settles it; assets count and are never moved; an
//! auction the app has no listing for, and consumables the app does not
//! hold, are held back; a refused group member refuses its group; and a
//! record a standing reconciliation made is undone only with it.

use std::sync::Arc;

use super::reading::{Container, Reading};
use super::reconcile::{plan_from_paste, plan_from_reading, PlanError};
use super::store::{record_reading, Purpose};
use super::test_support::reading;
use crate::analytics::{
    AccountRecord, AccountStep, AnalyticsError, AnalyticsService, ApplyOutcome, Caution,
    Disposition, Exclusion, LineChoice, LineRefusal, Profession, ReconciliationDraft,
    ReconciliationUndo, Revision, HELD_BY_RECONCILIATION, SESSION_TRACKED,
};
use crate::clock::MockClock;
use crate::db::Db;
use crate::time::naive_to_epoch;

const C: Container = Container::Carried;

fn noon_naive() -> chrono::NaiveDateTime {
    chrono::NaiveDateTime::parse_from_str("2026-06-01T12:00:00", "%Y-%m-%dT%H:%M:%S").unwrap()
}

/// The analytics clock, as the app reads it.
fn noon() -> f64 {
    naive_to_epoch(noon_naive())
}

/// Ten minutes before the clock: when the readings below were taken.
fn observed() -> f64 {
    noon() - 600.0
}

struct Env {
    _dir: tempfile::TempDir,
    db: Db,
    analytics: AnalyticsService,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("reconcile.db")).await.unwrap();
    let clock = Arc::new(MockClock::new(Some(noon_naive()), 0.0));
    let analytics = AnalyticsService::new(db.clone(), clock);
    Env {
        _dir: dir,
        db,
        analytics,
    }
}

/// Hunted loot of `item` held as stock, from a session of the hunting
/// definition `definition`.
async fn hunted(db: &Db, definition: i64, item: &'static str, quantity: i64, tt: f64) {
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO session_definitions(id, name, ad_hoc_segments, is_active) \
             VALUES(?1, 'Hunt ' || ?1, 0, 1)",
            [definition],
        )?;
        let session = format!("hunt-{definition}");
        conn.execute(
            "INSERT OR IGNORE INTO tracking_sessions(id, started_at, ended_at, is_active, \
             definition_id) VALUES(?1, 1000.0, 2000.0, 0, ?2)",
            rusqlite::params![session, definition],
        )?;
        let kill = format!("{session}-{item}");
        conn.execute(
            "INSERT INTO kills(id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
             shots_fired, damage_dealt, cost_ped, loot_total_ped, is_global, is_hof) \
             VALUES(?1, ?2, 'Atrox', 'Atrox', '', 1500.0, 1, 1.0, 0.1, ?3, 0, 0)",
            rusqlite::params![kill, session, tt],
        )?;
        conn.execute(
            "INSERT INTO kill_loot_items(kill_id, item_name, quantity, value_ped) \
             VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![kill, item, quantity, tt],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

async fn store(db: &Db, reading: Reading) {
    db.with_writer(move |conn| {
        record_reading(conn, &reading, Purpose::Ambient, None).expect("stored");
        Ok(())
    })
    .await
    .unwrap();
}

/// Plan from `rows` read ten minutes ago.
async fn plan(env: &Env, rows: &[(&str, Container, i64, i64)]) -> ReconciliationDraft {
    plan_at(env, observed(), rows).await
}

/// Plan from `rows` read at `at`.
async fn plan_at(env: &Env, at: f64, rows: &[(&str, Container, i64, i64)]) -> ReconciliationDraft {
    store(&env.db, reading(at, 0, rows)).await;
    plan_from_reading(&env.db, &env.analytics, None)
        .await
        .unwrap()
}

async fn held(env: &Env, item: &str) -> f64 {
    env.analytics
        .stock_positions(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.item_name == item)
        .map_or(0.0, |row| row.quantity)
}

async fn query_f64(db: &Db, sql: &'static str) -> f64 {
    db.with_reader(move |conn| Ok(conn.query_row(sql, [], |row| row.get::<_, f64>(0))?))
        .await
        .unwrap()
}

async fn ledger(db: &Db) -> Vec<(String, String, f64)> {
    db.with_reader(|conn| {
        let mut stmt = conn.prepare(
            // id-order: insertion (a test reading what it just wrote)
            "SELECT type, description, amount FROM ledger_entries ORDER BY rowid",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .await
    .unwrap()
}

async fn apply(env: &Env, draft: &ReconciliationDraft) -> ApplyOutcome {
    env.analytics.apply_reconciliation(&draft.id).await.unwrap()
}

fn applied_id(outcome: &ApplyOutcome) -> String {
    match outcome {
        ApplyOutcome::Applied {
            reconciliation_id: Some(id),
            ..
        } => id.clone(),
        other => panic!("expected an applied reconciliation, got {other:?}"),
    }
}

fn disposition(draft: &ReconciliationDraft, item: &str) -> Disposition {
    draft
        .lines
        .iter()
        .find(|line| line.item_name == item)
        .unwrap_or_else(|| panic!("a line for {item}: {draft:?}"))
        .disposition
        .clone()
}

async fn set(
    env: &Env,
    draft: &ReconciliationDraft,
    items: &[&str],
    choice: LineChoice,
) -> ReconciliationDraft {
    env.analytics
        .revise_reconciliation(
            &draft.id,
            Revision::Set {
                items: items.iter().map(|item| item.to_string()).collect(),
                choice,
            },
        )
        .await
        .unwrap()
        .draft
}

/// Bring previously accepted lines back into review.
async fn reconsider(env: &Env, draft: &ReconciliationDraft, items: &[&str]) -> ReconciliationDraft {
    env.analytics
        .revise_reconciliation(
            &draft.id,
            Revision::Reconsider {
                items: items.iter().map(|item| item.to_string()).collect(),
            },
        )
        .await
        .unwrap()
        .draft
}

async fn group(env: &Env, draft: &ReconciliationDraft, items: &[&str]) -> ReconciliationDraft {
    env.analytics
        .revise_reconciliation(
            &draft.id,
            Revision::Group {
                items: items.iter().map(|item| item.to_string()).collect(),
            },
        )
        .await
        .unwrap()
        .draft
}

/// 1000 PED of Shrapnel held as 600 from one hunt and 400 from another.
async fn sixty_forty(env: &Env) {
    hunted(&env.db, 5, "Shrapnel", 6_000_000, 600.0).await;
    hunted(&env.db, 6, "Shrapnel", 4_000_000, 400.0).await;
}

#[tokio::test]
async fn a_shortfall_leaves_each_source_in_proportion_and_touches_no_figure_of_play() {
    let env = env().await;
    sixty_forty(&env).await;
    // The account shows 900.
    let draft = plan(&env, &[("Shrapnel", C, 9_000_000, 90_000)]).await;
    assert_eq!(draft.lines.len(), 1);
    assert_eq!(draft.lines[0].app_quantity, 10_000_000.0);
    assert_eq!(draft.lines[0].observed_quantity, 9_000_000.0);
    let draft = set(&env, &draft, &["Shrapnel"], LineChoice::Removed).await;
    applied_id(&apply(&env, &draft).await);

    // A drops by 60, B by 40.
    let by_source: Vec<(i64, f64)> = env
        .db
        .with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT session_definition_id, SUM(quantity) FROM stock_movements \
                 WHERE movement_kind = 'removal' GROUP BY session_definition_id \
                 ORDER BY session_definition_id",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
        .unwrap();
    assert_eq!(by_source.len(), 2);
    assert_eq!(by_source[0].0, 5);
    assert!((by_source[0].1 + 600_000.0).abs() < 1e-6, "{by_source:?}");
    assert_eq!(by_source[1].0, 6);
    assert!((by_source[1].1 + 400_000.0).abs() < 1e-6, "{by_source:?}");
    assert!((held(&env, "Shrapnel").await - 9_000_000.0).abs() < 1e-6);
    // Loot stays as recorded, and no money moved.
    let looted = query_f64(&env.db, "SELECT SUM(quantity) FROM kill_loot_items").await;
    assert!((looted - 10_000_000.0).abs() < 1e-6);
    assert!(ledger(&env.db).await.is_empty());
}

#[tokio::test]
async fn missing_shrapnel_is_converted_by_default_and_realises_the_terminal_s_percent() {
    let env = env().await;
    sixty_forty(&env).await;
    let draft = plan(&env, &[("Shrapnel", C, 9_000_000, 90_000)]).await;
    assert_eq!(
        disposition(&draft, "Shrapnel"),
        Disposition::ShrapnelConverted { sold_at_tt: None }
    );
    applied_id(&apply(&env, &draft).await);
    assert!((held(&env, "Shrapnel").await - 9_000_000.0).abs() < 1e-6);
    let ledger = ledger(&env.db).await;
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0].0, "markup");
    assert!((ledger[0].2 - 1.0).abs() < 1e-9, "{ledger:?}");
}

#[tokio::test]
async fn a_surplus_is_of_unknown_origin_and_its_later_sale_books_only_its_markup() {
    let env = env().await;
    // Nanocubes are 0.01 PED each, a constant of the game.
    let draft = plan(&env, &[("Nanocube", C, 500, 500)]).await;
    assert_eq!(disposition(&draft, "Nanocube"), Disposition::UnknownOrigin);
    applied_id(&apply(&env, &draft).await);
    assert!((held(&env, "Nanocube").await - 500.0).abs() < 1e-9);
    // Its TT is never income.
    assert!(ledger(&env.db).await.is_empty());

    env.analytics
        .create_private_sale(Profession::Inventory, "Nanocube", 500.0, 8.0, None)
        .await
        .unwrap();
    let ledger = ledger(&env.db).await;
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0].0, "markup");
    assert!((ledger[0].2 - 3.0).abs() < 1e-9, "{ledger:?}");
    // Credited to no activity.
    let attributed = query_f64(
        &env.db,
        "SELECT attributed_qty FROM private_sales WHERE item_name = 'Nanocube'",
    )
    .await;
    assert_eq!(attributed, 0.0);
}

#[tokio::test]
async fn stock_of_unknown_origin_shows_in_the_inventory_s_history_with_its_undo() {
    let env = env().await;
    // Held as of unknown origin by a check, and from an arrival the
    // readings saw.
    let draft = plan(&env, &[("Nanocube", C, 500, 500)]).await;
    let check = applied_id(&apply(&env, &draft).await);
    let arrival = env
        .analytics
        .record_unknown_origin("Iron Stone", 3.0, 3.9, noon(), None)
        .await
        .unwrap()
        .record_id;
    let history = env
        .analytics
        .activity_history(Profession::Inventory)
        .await
        .unwrap();
    let row = |item: &str| {
        history
            .iter()
            .find(|row| row.item_name == item)
            .unwrap_or_else(|| panic!("a history row for {item}: {history:?}"))
            .clone()
    };
    // The check's is undone only with the check.
    let by_check = row("Nanocube");
    assert_eq!(by_check.kind, "unknownOrigin");
    assert_eq!(by_check.reconciliation_id.as_deref(), Some(check.as_str()));
    assert!((by_check.quantity - 500.0).abs() < 1e-9);
    assert!((by_check.tt_value - 5.0).abs() < 1e-9);
    assert!(!by_check.can_delete);
    assert_eq!(
        by_check.undo_blocked_reason.as_deref(),
        Some(HELD_BY_RECONCILIATION)
    );
    // The arrival's is undone on its own, and stays as the record of it.
    let by_arrival = row("Iron Stone");
    assert_eq!(by_arrival.id, arrival);
    assert_eq!(by_arrival.reconciliation_id, None);
    assert!(by_arrival.can_delete);
    assert!(!by_arrival.undone);
    env.analytics.undo_unknown_origin(&arrival).await.unwrap();
    let undone = env
        .analytics
        .activity_history(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == arrival)
        .expect("kept once undone");
    assert!(undone.undone);
    assert!(!undone.can_delete);
    // It belongs to no activity: no activity's history shows it.
    assert!(env
        .analytics
        .activity_history(Profession::Hunting)
        .await
        .unwrap()
        .iter()
        .all(|row| row.kind != "unknownOrigin"));
}

#[tokio::test]
async fn a_check_that_only_leaves_differences_records_nothing_but_keeps_them_accepted() {
    let env = env().await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    let draft = plan(&env, &[("Iron Stone", C, 6, 78)]).await;
    let draft = set(&env, &draft, &["Iron Stone"], LineChoice::LeftAsIs).await;
    let outcome = apply(&env, &draft).await;
    applied_id(&outcome);
    assert!(matches!(
        &outcome,
        ApplyOutcome::Applied { settled, refused, .. }
            if settled == &["Iron Stone".to_string()] && refused.is_empty()
    ));
    // Nothing moved: the check is kept only to remember the player's word.
    assert!((held(&env, "Iron Stone").await - 10.0).abs() < 1e-9);
    let kept = query_f64(&env.db, "SELECT COUNT(*) * 1.0 FROM stock_reconciliations").await;
    assert_eq!(kept, 1.0);
}

#[tokio::test]
async fn a_difference_left_as_it_is_is_filed_apart_until_it_moves_or_is_reconsidered() {
    let env = env().await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    let first = plan_at(&env, noon(), &[("Iron Stone", C, 6, 78)]).await;
    let first = set(&env, &first, &["Iron Stone"], LineChoice::LeftAsIs).await;
    let id = applied_id(&apply(&env, &first).await);

    // The same difference again: filed apart as accepted, with when.
    let same = plan_at(&env, noon() + 60.0, &[("Iron Stone", C, 6, 78)]).await;
    assert!(same.lines.iter().all(|line| line.item_name != "Iron Stone"));
    let accepted = same
        .previously_accepted
        .iter()
        .find(|line| line.item_name == "Iron Stone")
        .expect("filed apart");
    assert!(accepted.accepted_at.is_some());
    // Brought back into review, it starts as any other line.
    let brought = reconsider(&env, &same, &["Iron Stone"]).await;
    assert_eq!(disposition(&brought, "Iron Stone"), Disposition::Removed);
    assert!(brought.previously_accepted.is_empty());

    // A difference that moved since is a live line.
    let moved = plan_at(&env, noon() + 120.0, &[("Iron Stone", C, 5, 65)]).await;
    assert_eq!(disposition(&moved, "Iron Stone"), Disposition::Removed);
    assert!(moved.previously_accepted.is_empty());

    // Undoing the check forgets what it accepted.
    assert_eq!(
        env.analytics.undo_reconciliation(&id).await.unwrap(),
        ReconciliationUndo::Undone
    );
    let after = plan_at(&env, noon() + 180.0, &[("Iron Stone", C, 6, 78)]).await;
    assert_eq!(disposition(&after, "Iron Stone"), Disposition::Removed);
    assert!(after.previously_accepted.is_empty());
}

#[tokio::test]
async fn extra_units_valued_only_by_the_account_s_figures_are_held_at_them() {
    let env = env().await;
    let draft = plan(&env, &[("Garcen Grease", C, 10, 200)]).await;
    // The account's own figure is the game's: the extra units are held at
    // it, of unknown origin.
    assert_eq!(
        disposition(&draft, "Garcen Grease"),
        Disposition::UnknownOrigin
    );
    applied_id(&apply(&env, &draft).await);
    assert!((held(&env, "Garcen Grease").await - 10.0).abs() < 1e-9);
}

#[tokio::test]
async fn a_conversion_group_records_one_recycling() {
    let env = env().await;
    hunted(&env.db, 5, "Animal Oil Residue", 100, 1.0).await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    let draft = plan(&env, &[("Nanocube", C, 230, 230)]).await;
    assert_eq!(draft.lines.len(), 3);
    // The largest difference by TT first.
    assert_eq!(draft.lines[0].item_name, "Nanocube");
    let draft = group(
        &env,
        &draft,
        &["Animal Oil Residue", "Iron Stone", "Nanocube"],
    )
    .await;
    assert_eq!(draft.groups.len(), 1);
    assert!((draft.groups[0].tt_in - 2.3).abs() < 1e-9);
    assert!((draft.groups[0].tt_out - 2.3).abs() < 1e-9);
    applied_id(&apply(&env, &draft).await);
    // Balanced: no TT appeared or vanished.
    assert!(ledger(&env.db).await.is_empty());
    let conversions: Vec<_> = env
        .analytics
        .activity_history(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.kind == "conversion")
        .collect();
    assert_eq!(conversions.len(), 1);
    assert!((held(&env, "Nanocube").await - 230.0).abs() < 1e-6);
    assert!(held(&env, "Animal Oil Residue").await.abs() < 1e-9);
    assert!(held(&env, "Iron Stone").await.abs() < 1e-9);
    let unknown = query_f64(&env.db, "SELECT COUNT(*) * 1.0 FROM stock_unknown_origins").await;
    assert_eq!(unknown, 0.0);
}

#[tokio::test]
async fn ammo_equipment_attached_and_auction_rows_never_enter_the_plan() {
    let env = env().await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    env.db
        .with_writer(|conn| {
            conn.execute(
                "INSERT INTO equipment_library (name, item_type, properties_json) \
                 VALUES ('Opalo', 'weapon', '{}')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let draft = plan(
        &env,
        &[
            ("Universal Ammo", C, 500_000, 5_000),
            ("Opalo", C, 1, 400),
            (
                "Armor Plating Mk. 6A",
                Container::Attached {
                    parent: "Pegasus Harness".to_string(),
                },
                1,
                2_500,
            ),
            ("Pegasus Harness", C, 1, 1_200),
            ("Iron Stone", C, 10, 130),
            ("Iron Stone", Container::Auction, 3, 39),
        ],
    )
    .await;
    // The ten held match the ten carried; the three listed do not count.
    assert!(draft.lines.is_empty(), "{:?}", draft.lines);
    let reasons: Vec<(String, Exclusion)> = draft
        .excluded
        .iter()
        .map(|item| (item.item_name.clone(), item.reason))
        .collect();
    for expected in [
        ("Universal Ammo", Exclusion::UniversalAmmo),
        ("Opalo", Exclusion::Equipment),
        ("Armor Plating Mk. 6A", Exclusion::Attached),
        ("Iron Stone", Exclusion::Auction),
    ] {
        assert!(
            reasons
                .iter()
                .any(|(name, reason)| name == expected.0 && *reason == expected.1),
            "{expected:?} in {reasons:?}"
        );
    }
}

#[tokio::test]
async fn applying_twice_changes_nothing() {
    let env = env().await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    let draft = plan(&env, &[("Iron Stone", C, 6, 78)]).await;
    let id = applied_id(&apply(&env, &draft).await);
    assert!((held(&env, "Iron Stone").await - 6.0).abs() < 1e-9);
    assert_eq!(
        apply(&env, &draft).await,
        ApplyOutcome::AlreadyApplied {
            reconciliation_id: id
        }
    );
    assert!((held(&env, "Iron Stone").await - 6.0).abs() < 1e-9);
    // Planned again from the same reading, there is nothing left to settle.
    let again = plan_from_reading(&env.db, &env.analytics, None)
        .await
        .unwrap();
    assert!(again.lines.is_empty(), "{:?}", again.lines);
    let removals = query_f64(&env.db, "SELECT COUNT(*) * 1.0 FROM stock_removals").await;
    assert_eq!(removals, 1.0);
}

#[tokio::test]
async fn a_line_whose_item_moved_after_the_reading_is_refused_and_the_rest_applied() {
    let env = env().await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    hunted(&env.db, 5, "Lysterium Stone", 10, 1.0).await;
    store(
        &env.db,
        reading(
            observed(),
            0,
            &[("Iron Stone", C, 6, 78), ("Lysterium Stone", C, 7, 70)],
        ),
    )
    .await;
    // Recorded at the clock, after the reading was taken.
    env.analytics
        .remove_stock(Profession::Inventory, "Iron Stone", 2.0, None)
        .await
        .unwrap();
    let draft = plan_from_reading(&env.db, &env.analytics, None)
        .await
        .unwrap();
    let outcome = apply(&env, &draft).await;
    let ApplyOutcome::Applied {
        settled, refused, ..
    } = outcome
    else {
        panic!("applied in part: {outcome:?}");
    };
    assert_eq!(settled, vec!["Lysterium Stone".to_string()]);
    assert_eq!(
        refused,
        vec![("Iron Stone".to_string(), LineRefusal::StockMovedSince)]
    );
    assert!((held(&env, "Iron Stone").await - 8.0).abs() < 1e-9);
    assert!((held(&env, "Lysterium Stone").await - 7.0).abs() < 1e-9);
}

#[tokio::test]
async fn undo_restores_the_quantities_as_a_whole() {
    let env = env().await;
    sixty_forty(&env).await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    let draft = plan(
        &env,
        &[
            ("Shrapnel", C, 9_000_000, 90_000),
            ("Iron Stone", C, 4, 52),
            ("Nanocube", C, 1_000, 1_000),
        ],
    )
    .await;
    let id = applied_id(&apply(&env, &draft).await);
    assert!((held(&env, "Shrapnel").await - 9_000_000.0).abs() < 1e-6);
    assert!((held(&env, "Iron Stone").await - 4.0).abs() < 1e-9);
    assert!((held(&env, "Nanocube").await - 1_000.0).abs() < 1e-9);

    assert_eq!(
        env.analytics.undo_reconciliation(&id).await.unwrap(),
        ReconciliationUndo::Undone
    );
    assert!((held(&env, "Shrapnel").await - 10_000_000.0).abs() < 1e-6);
    assert!((held(&env, "Iron Stone").await - 10.0).abs() < 1e-9);
    assert!(held(&env, "Nanocube").await.abs() < 1e-9);
    // The conversion's realised percent goes with it.
    assert!(ledger(&env.db).await.is_empty());
    // The record stays, undone.
    let undone = query_f64(
        &env.db,
        "SELECT COUNT(*) * 1.0 FROM stock_reconciliations WHERE undone_at IS NOT NULL",
    )
    .await;
    assert_eq!(undone, 1.0);
    assert_eq!(
        env.analytics.undo_reconciliation(&id).await.unwrap(),
        ReconciliationUndo::NotFound
    );
}

#[tokio::test]
async fn a_pasted_inventory_and_a_reading_of_it_give_the_same_plan() {
    let env = env().await;
    sixty_forty(&env).await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    let from_reading = plan(
        &env,
        &[
            ("Shrapnel", C, 8_000_000, 80_000),
            (
                "Shrapnel",
                Container::Storage {
                    planet: "The Hub".to_string(),
                },
                1_000_000,
                10_000,
            ),
            ("Iron Stone", C, 4, 52),
            ("Garcen Grease", C, 10, 200),
            ("Universal Ammo", C, 500_000, 5_000),
            ("Gorgon Gloves (M)", Container::Auction, 1, 140),
            (
                "Armor Plating Mk. 6A",
                Container::Attached {
                    parent: "Pegasus Harness".to_string(),
                },
                1,
                2_500,
            ),
        ],
    )
    .await;
    let paste = "Name\tQuantity\tValue(PED)\tContainer\tContainerRefId\n\
                 Shrapnel\t8000000\t800.0000\tCARRIED\t\n\
                 Shrapnel\t1000000\t100.0000\tSTORAGE (The Hub\n)\t\n\
                 Iron Stone\t4\t0.5200\tCARRIED\t\n\
                 Garcen Grease\t10\t2.0000\tCARRIED\t\n\
                 Universal Ammo\t500000\t50.0000\tCARRIED\t\n\
                 Gorgon Gloves (M)\t1\t1.4000\tAUCTION\t\n\
                 Armor Plating Mk. 6A\t1\t25.0000\tPegasus Harness\t9\n";
    let from_paste = plan_from_paste(&env.db, &env.analytics, None, paste, noon())
        .await
        .unwrap();
    assert_eq!(from_paste.lines, from_reading.lines);
    assert_eq!(from_paste.excluded, from_reading.excluded);
    assert_eq!(from_reading.lines.len(), 3);
}

#[tokio::test]
async fn nothing_is_planned_or_applied_while_a_session_is_tracked() {
    let env = env().await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    let draft = plan(&env, &[("Iron Stone", C, 6, 78)]).await;
    env.db
        .with_writer(|conn| {
            conn.execute(
                "INSERT INTO tracking_sessions(id, started_at, is_active) \
                 VALUES('running', 1700000000.0, 1)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(apply(&env, &draft).await, ApplyOutcome::SessionTracked);
    assert!((held(&env, "Iron Stone").await - 10.0).abs() < 1e-9);
    let refused = plan_from_reading(&env.db, &env.analytics, None).await;
    assert!(
        matches!(
            &refused,
            Err(PlanError::Analytics(AnalyticsError::Rejected(reason))) if reason == SESSION_TRACKED
        ),
        "{refused:?}"
    );
}

#[tokio::test]
async fn an_item_taken_back_stays_left_as_it_is_until_a_reconciliation_settles_it() {
    let env = env().await;
    sixty_forty(&env).await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    // 10:00, the account records a Shrapnel conversion; 11:00, the player
    // takes it back.
    let step = AccountStep {
        reading_id: 2,
        prev_reading_id: 1,
        seen_from: noon() - 7_260.0,
        seen_to: noon() - 7_200.0,
        hand_entries_since: noon() - 7_560.0,
        stock_settles_by: noon() - 6_600.0,
        card_change_pec: None,
    };
    let crate::analytics::FromAccount::Recorded { record_id, .. } = env
        .analytics
        .record_from_account(
            step,
            AccountRecord::ShrapnelConversion {
                quantity: 1_000_000.0,
            },
        )
        .await
        .unwrap()
    else {
        panic!("recorded from the account");
    };
    assert!(env
        .analytics
        .undo_stock_conversion(&record_id)
        .await
        .unwrap());

    // 12:00, a reconciliation settles Iron Stone and leaves Shrapnel as it is.
    let twelve = plan_at(
        &env,
        noon() + 60.0,
        &[("Shrapnel", C, 9_000_000, 90_000), ("Iron Stone", C, 6, 78)],
    )
    .await;
    assert_eq!(disposition(&twelve, "Shrapnel"), Disposition::LeftAsIs);
    assert!(twelve
        .lines
        .iter()
        .any(|line| line.item_name == "Shrapnel" && line.taken_back == vec![record_id.clone()]));
    assert_eq!(disposition(&twelve, "Iron Stone"), Disposition::Removed);
    applied_id(&apply(&env, &twelve).await);
    assert!((held(&env, "Shrapnel").await - 10_000_000.0).abs() < 1e-6);

    // 13:00, the same difference: the player's word at 12:00 stands, filed
    // apart. Brought back into review it is still left as it is, whatever
    // came between; this time the player settles it.
    let thirteen = plan_at(&env, noon() + 120.0, &[("Shrapnel", C, 9_000_000, 90_000)]).await;
    assert!(thirteen
        .previously_accepted
        .iter()
        .any(|line| line.item_name == "Shrapnel"));
    let thirteen = reconsider(&env, &thirteen, &["Shrapnel"]).await;
    assert_eq!(disposition(&thirteen, "Shrapnel"), Disposition::LeftAsIs);
    let thirteen = set(&env, &thirteen, &["Shrapnel"], LineChoice::Removed).await;
    applied_id(&apply(&env, &thirteen).await);

    // 14:00, settled once: a new difference starts as any other.
    let fourteen = plan_at(&env, noon() + 180.0, &[("Shrapnel", C, 8_500_000, 85_000)]).await;
    assert_eq!(
        disposition(&fourteen, "Shrapnel"),
        Disposition::ShrapnelConverted { sold_at_tt: None }
    );
}

async fn asset(env: &Env, name: &str, quantity: f64, tt: f64) {
    env.analytics
        .create_inventory_item(name, Some(quantity), tt, 0.0, None, Some("2026-05-01"))
        .await
        .unwrap();
}

#[tokio::test]
async fn an_asset_counts_in_what_the_app_holds() {
    let only_asset = env().await;
    asset(&only_asset, "Garcen Grease", 50.0, 10.0).await;
    let draft = plan(&only_asset, &[("Garcen Grease", C, 50, 1_000)]).await;
    assert!(draft.lines.is_empty(), "{:?}", draft.lines);

    let both = env().await;
    hunted(&both.db, 5, "Garcen Grease", 100, 20.0).await;
    asset(&both, "Garcen Grease", 500.0, 100.0).await;
    let draft = plan(&both, &[("Garcen Grease", C, 600, 12_000)]).await;
    assert!(draft.lines.is_empty(), "{:?}", draft.lines);
}

#[tokio::test]
async fn a_shortfall_beyond_the_loose_stock_is_held_back_as_an_asset_s() {
    let env = env().await;
    hunted(&env.db, 5, "Garcen Grease", 100, 20.0).await;
    asset(&env, "Garcen Grease", 500.0, 100.0).await;
    // Read after the asset was bought. 150 missing, 100 loose.
    let draft = plan_at(&env, noon() + 60.0, &[("Garcen Grease", C, 450, 9_000)]).await;
    assert!(draft.lines.is_empty(), "{:?}", draft.lines);
    assert!(draft
        .excluded
        .iter()
        .any(|item| item.item_name == "Garcen Grease" && item.reason == Exclusion::HeldAsAsset));
    // Within the loose stock, the removal takes it from there alone.
    let draft = plan_at(&env, noon() + 120.0, &[("Garcen Grease", C, 550, 11_000)]).await;
    assert_eq!(disposition(&draft, "Garcen Grease"), Disposition::Removed);
    applied_id(&apply(&env, &draft).await);
    assert!((held(&env, "Garcen Grease").await - 50.0).abs() < 1e-9);
    let assets = env.analytics.list_inventory().await.unwrap();
    assert!((assets[0].quantity - 500.0).abs() < 1e-9);
}

#[tokio::test]
async fn more_at_auction_than_the_app_has_listed_holds_the_item_back() {
    let env = env().await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    let draft = plan(
        &env,
        &[
            ("Iron Stone", C, 7, 91),
            ("Iron Stone", Container::Auction, 3, 39),
        ],
    )
    .await;
    assert!(draft.lines.is_empty(), "{:?}", draft.lines);
    assert!(draft
        .excluded
        .iter()
        .any(|item| item.item_name == "Iron Stone"
            && item.reason == Exclusion::AuctionWithoutListing
            && (item.quantity - 3.0).abs() < 1e-9));
}

#[tokio::test]
async fn a_consumable_is_compared_only_where_the_app_holds_it_as_stock() {
    let env = env().await;
    hunted(&env.db, 5, "Mind Essence", 100, 1.0).await;
    env.db
        .with_writer(|conn| {
            conn.execute(
                "INSERT INTO equipment_library (name, item_type, properties_json)                  VALUES ('Herb Box', 'consumable', '{}')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let draft = plan(
        &env,
        &[("Mind Essence", C, 60, 60), ("Herb Box", C, 5, 250)],
    )
    .await;
    assert_eq!(disposition(&draft, "Mind Essence"), Disposition::Removed);
    assert!(draft
        .excluded
        .iter()
        .any(|item| item.item_name == "Herb Box" && item.reason == Exclusion::Consumable));
}

#[tokio::test]
async fn a_line_whose_holding_changed_since_the_plan_is_refused() {
    let env = env().await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    env.analytics
        .remove_stock(Profession::Inventory, "Iron Stone", 2.0, None)
        .await
        .unwrap();
    let removal = env
        .analytics
        .activity_history(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.kind == "removal")
        .expect("the removal")
        .id;
    // Read after the removal: the app holds 8, the account shows 6.
    let draft = plan_at(&env, noon() + 60.0, &[("Iron Stone", C, 6, 78)]).await;
    assert_eq!(draft.lines[0].app_quantity, 8.0);
    // Taken back: no movement is recorded, but the holding is 10 again.
    assert!(env.analytics.undo_stock_removal(&removal).await.unwrap());
    let outcome = apply(&env, &draft).await;
    assert_eq!(
        outcome,
        ApplyOutcome::Applied {
            reconciliation_id: None,
            settled: Vec::new(),
            refused: vec![("Iron Stone".to_string(), LineRefusal::ChangedSincePlanned)],
        }
    );
    assert!((held(&env, "Iron Stone").await - 10.0).abs() < 1e-9);
}

#[tokio::test]
async fn one_refused_member_refuses_its_whole_conversion() {
    let env = env().await;
    hunted(&env.db, 5, "Animal Oil Residue", 100, 1.0).await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    let draft = plan(&env, &[("Nanocube", C, 230, 230)]).await;
    let draft = group(
        &env,
        &draft,
        &["Animal Oil Residue", "Iron Stone", "Nanocube"],
    )
    .await;
    // Recorded after the reading.
    env.analytics
        .remove_stock(Profession::Inventory, "Animal Oil Residue", 1.0, None)
        .await
        .unwrap();
    let ApplyOutcome::Applied {
        reconciliation_id,
        refused,
        ..
    } = apply(&env, &draft).await
    else {
        panic!("applied in part");
    };
    assert_eq!(reconciliation_id, None);
    let reasons: std::collections::BTreeMap<String, LineRefusal> = refused.into_iter().collect();
    assert_eq!(
        reasons.get("Animal Oil Residue"),
        Some(&LineRefusal::StockMovedSince)
    );
    assert_eq!(reasons.get("Iron Stone"), Some(&LineRefusal::GroupRefused));
    assert_eq!(reasons.get("Nanocube"), Some(&LineRefusal::GroupRefused));
    assert!(held(&env, "Nanocube").await.abs() < 1e-9);
    assert!((held(&env, "Iron Stone").await - 10.0).abs() < 1e-9);
}

#[tokio::test]
async fn a_reconciliation_whose_product_was_used_is_not_undone_nor_its_records_alone() {
    let env = env().await;
    hunted(&env.db, 5, "Animal Oil Residue", 100, 1.0).await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    let draft = plan(&env, &[("Nanocube", C, 230, 230)]).await;
    let draft = group(
        &env,
        &draft,
        &["Animal Oil Residue", "Iron Stone", "Nanocube"],
    )
    .await;
    let id = applied_id(&apply(&env, &draft).await);

    // History names the reconciliation, and its entry is undone only with it.
    let conversion = env
        .analytics
        .activity_history(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.kind == "conversion")
        .expect("the conversion");
    assert_eq!(conversion.reconciliation_id.as_deref(), Some(id.as_str()));
    assert!(!conversion.can_delete);
    assert!(matches!(
        env.analytics.undo_stock_conversion(&conversion.id).await,
        Err(AnalyticsError::Rejected(reason)) if reason == HELD_BY_RECONCILIATION
    ));

    // The Nanocubes it made are sold: taking it back would leave less than
    // nothing of them.
    env.analytics
        .create_private_sale(Profession::Inventory, "Nanocube", 230.0, 2.3, None)
        .await
        .unwrap();
    assert!(matches!(
        env.analytics.undo_reconciliation(&id).await.unwrap(),
        ReconciliationUndo::Blocked(_)
    ));
    assert!(held(&env, "Iron Stone").await.abs() < 1e-9);
}

#[tokio::test]
async fn only_the_converted_part_of_missing_shrapnel_realises_the_terminal_s_percent() {
    let env = env().await;
    sixty_forty(&env).await;
    let draft = plan(&env, &[("Shrapnel", C, 9_000_000, 90_000)]).await;
    let draft = env
        .analytics
        .revise_reconciliation(
            &draft.id,
            Revision::ShrapnelSoldAtTt {
                quantity: Some(400_000.0),
            },
        )
        .await
        .unwrap()
        .draft;
    applied_id(&apply(&env, &draft).await);
    // 600,000 converted (60 PED) realise 0.60; the 400,000 sold at TT
    // realise nothing.
    let ledger = ledger(&env.db).await;
    assert_eq!(ledger.len(), 1, "{ledger:?}");
    assert_eq!(ledger[0].0, "markup");
    assert_eq!(ledger[0].1, "Shrapnel Conversion");
    assert!((ledger[0].2 - 0.6).abs() < 1e-9, "{ledger:?}");
    assert!((held(&env, "Shrapnel").await - 9_000_000.0).abs() < 1e-6);
}

#[tokio::test]
async fn a_paste_planned_soon_after_a_session_says_to_copy_it_afresh() {
    let env = env().await;
    hunted(&env.db, 5, "Iron Stone", 10, 1.3).await;
    let paste = "Name\tQuantity\tValue(PED)\tContainer\tContainerRefId\n\
                 Iron Stone\t6\t0.7800\tCARRIED\t\n";
    let calm = plan_from_paste(&env.db, &env.analytics, None, paste, noon())
        .await
        .unwrap();
    assert_eq!(calm.caution, None);
    let ended = noon() - 600.0;
    env.db
        .with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions(id, started_at, ended_at, is_active) \
                 VALUES('just-ended', ?1, ?2, 0)",
                rusqlite::params![ended - 3_600.0, ended],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let soon = plan_from_paste(&env.db, &env.analytics, None, paste, noon())
        .await
        .unwrap();
    assert_eq!(
        soon.caution,
        Some(Caution::PasteSoonAfterSession {
            session_ended_at: ended
        })
    );
}
