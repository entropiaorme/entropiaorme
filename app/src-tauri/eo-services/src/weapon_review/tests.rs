//! Weapon corrections: an assignment's effect, its exact undo, the refusals,
//! the review reads, and conservation over generated sequences.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::*;
use crate::clock::MockClock;

/// A pistol costing 0.05 PED a shot (5 PEC decay), band 5-10.
const PISTOL: i64 = 1;
/// A cannon costing 0.2 PED a shot, band 20-40.
const CANNON: i64 = 2;
/// Not a weapon.
const FAP: i64 = 3;
/// A weapon in Equipment that no seeded shot was fired while carrying.
const RIFLE: i64 = 4;
/// Carried when the shots landed, since removed from Equipment.
const SOLD_RIFLE: i64 = 99;

struct Harness {
    _dir: tempfile::TempDir,
    db: Db,
    service: WeaponReviewService,
    announced: Arc<AtomicUsize>,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("entropia_orme.db"))
        .await
        .unwrap();
    let clock = Arc::new(MockClock::new(None, 0.0));
    let announced = Arc::new(AtomicUsize::new(0));
    let counter = announced.clone();
    let service = WeaponReviewService::new(db.clone(), clock).with_changed(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    db.with_writer(|conn| {
        conn.execute_batch(
            r#"INSERT INTO equipment_library (id, name, item_type, properties_json) VALUES
               (1, 'Pistol', 'weapon',
                '{"weapon_entity": {"damage": {"impact": 10}, "economy": {"decay": 5.0, "ammo_burn": 0}}}'),
               (2, 'Cannon', 'weapon',
                '{"weapon_entity": {"damage": {"impact": 40}, "economy": {"decay": 20.0, "ammo_burn": 0}}}'),
               (3, 'FAP', 'healing', '{}'),
               (4, 'Rifle', 'weapon',
                '{"weapon_entity": {"damage": {"impact": 16}, "economy": {"decay": 10.0, "ammo_burn": 0}}}');"#,
        )?;
        Ok(())
    })
    .await
    .unwrap();
    Harness {
        _dir: dir,
        db,
        service,
        announced,
    }
}

fn candidates() -> String {
    serde_json::json!([
        {"equipmentId": PISTOL, "name": "Pistol", "fits": false},
        {"equipmentId": CANNON, "name": "Cannon", "fits": true},
        {"equipmentId": SOLD_RIFLE, "name": "Sold rifle", "fits": true},
    ])
    .to_string()
}

/// One session: a kill with a pistol phase of two shots and an unpriced
/// phase of two shots (u1 at 25, u2 countered), one unpriced shot after
/// the last kill (u3 at 30, dangling), one evidence shot, and one live
/// decision.
async fn seed_session(db: &Db, session_id: &'static str, active: bool) {
    let candidates = candidates();
    db.with_writer(move |conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO tracking_sessions \
             (id, started_at, ended_at, is_active, dangling_cost, weapon_shots_agreed, \
              weapon_shots_evidenced) VALUES (?1, 1000, ?2, ?3, 0, 2, 1)",
            rusqlite::params![session_id, (!active).then_some(2000.0), active as i64],
        )?;
        let id = |name: &str| format!("{session_id}-{name}");
        tx.execute(
            "INSERT INTO kills (id, session_id, mob_name, timestamp, shots_fired, cost_ped) \
             VALUES (?1, ?2, 'Atrox', 1500, 5, 0.3)",
            rusqlite::params![id("k"), session_id],
        )?;
        tx.execute(
            "INSERT INTO kill_tool_stats \
             (kill_id, tool_name, shots_fired, damage_dealt, critical_hits, cost_per_shot, \
              expected_economics_json, evidence_fingerprint) VALUES \
             (?1, 'Pistol', 2, 14, 0, 0.05, NULL, ''), \
             (?1, 'Unknown', 2, 25, 1, 0, NULL, ''), \
             (?1, 'Cannon', 1, 30, 0, 0.2, NULL, '')",
            [id("k")],
        )?;
        tx.execute(
            "INSERT INTO weapon_attribution_reviews \
             (id, session_id, decision, hotbar_tool, evidence_tool, mismatch_since, decided_at, \
              repriced_shots, cost_delta_ped) \
             VALUES (?1, ?2, 'kept', 'Pistol', 'Cannon', 1400, 1450, 0, 0)",
            rusqlite::params![id("r"), session_id],
        )?;
        for (name, kill, at, amount, critical, attribution, tool, cost) in [
            (
                "u1",
                Some("k"),
                1300.0,
                Some(25.0),
                true,
                "unresolved",
                None,
                0.0,
            ),
            (
                "u2",
                Some("k"),
                1310.0,
                None,
                false,
                "unresolved",
                None,
                0.0,
            ),
            (
                "u3",
                None,
                1600.0,
                Some(30.0),
                false,
                "unresolved",
                None,
                0.0,
            ),
            (
                "e1",
                Some("k"),
                1400.0,
                Some(30.0),
                false,
                "evidence",
                Some("Cannon"),
                0.2,
            ),
        ] {
            tx.execute(
                "INSERT INTO weapon_shot_evidence \
                 (id, session_id, kill_id, observed_at, amount, critical, attribution, \
                  hotbar_tool, tool_name, cost_per_shot, candidates_json, reason, review_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'Pistol', ?8, ?9, ?10, 'seeded', ?11)",
                rusqlite::params![
                    id(name),
                    session_id,
                    kill.map(id),
                    at,
                    amount,
                    critical,
                    attribution,
                    tool,
                    cost,
                    candidates,
                    (name == "e1").then(|| id("r")),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
    .await
    .unwrap();
}

/// The kill's phases as (tool, shots, damage, crits, cost), and its cost.
async fn kill_state(db: &Db, kill_id: &'static str) -> (Vec<(String, i64, f64, i64, f64)>, f64) {
    db.with_reader(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT tool_name, shots_fired, damage_dealt, critical_hits, cost_per_shot \
             FROM kill_tool_stats WHERE kill_id = ?1 ORDER BY tool_name, cost_per_shot",
        )?;
        let phases = stmt
            .query_map([kill_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let cost = conn.query_row(
            "SELECT cost_ped FROM kills WHERE id = ?1",
            [kill_id],
            |row| row.get(0),
        )?;
        Ok((phases, cost))
    })
    .await
    .unwrap()
}

async fn scalar_f64(db: &Db, sql: &'static str) -> f64 {
    db.with_reader(move |conn| Ok(conn.query_row(sql, [], |row| row.get(0))?))
        .await
        .unwrap()
}

async fn scalar_i64(db: &Db, sql: &'static str) -> i64 {
    db.with_reader(move |conn| Ok(conn.query_row(sql, [], |row| row.get(0))?))
        .await
        .unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[tokio::test]
async fn assigning_a_settled_shot_moves_it_into_the_weapons_phase() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    let correction = h.service.assign("s-u1", CANNON).await.unwrap();
    assert_eq!(correction.session_id, "s");
    assert_eq!(correction.tool_name, "Cannon");
    assert!(close(correction.cost_per_shot, 0.2));
    assert_eq!(h.announced.load(Ordering::SeqCst), 1);

    let (phases, cost) = kill_state(&h.db, "s-k").await;
    assert_eq!(
        phases,
        vec![
            ("Cannon".to_string(), 2, 55.0, 1, 0.2),
            ("Pistol".to_string(), 2, 14.0, 0, 0.05),
            ("Unknown".to_string(), 1, 0.0, 0, 0.0),
        ]
    );
    assert!(close(cost, 0.5));
    assert_eq!(
        scalar_i64(
            &h.db,
            "SELECT COUNT(*) FROM weapon_shot_evidence \
             WHERE id = 's-u1' AND tool_name = 'Cannon' AND cost_per_shot = 0.2 \
               AND correction_id IS NOT NULL"
        )
        .await,
        1
    );
    // The session's summary follows the kill's cost.
    assert!(close(
        scalar_f64(
            &h.db,
            "SELECT weapon_cost FROM session_summaries WHERE session_id = 's'",
        )
        .await,
        0.5
    ));

    let block =
        h.db.with_reader(|conn| session_detail_block(conn, "s"))
            .await
            .unwrap();
    assert_eq!(block["unresolved"], 3);
    assert_eq!(block["unpriced"], 2);
    assert_eq!(block["assigned"], 1);
}

#[tokio::test]
async fn undoing_an_assignment_restores_the_shot_exactly() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    let before = kill_state(&h.db, "s-k").await;
    let correction = h.service.assign("s-u1", PISTOL).await.unwrap();
    assert_ne!(kill_state(&h.db, "s-k").await, before);
    assert_eq!(h.service.undo(&correction.id).await.unwrap(), "s");
    assert_eq!(kill_state(&h.db, "s-k").await, before);
    assert_eq!(
        scalar_i64(
            &h.db,
            "SELECT COUNT(*) FROM weapon_shot_evidence \
             WHERE id = 's-u1' AND tool_name IS NULL AND cost_per_shot = 0 \
               AND correction_id IS NULL"
        )
        .await,
        1
    );
    assert_eq!(
        scalar_i64(
            &h.db,
            "SELECT COUNT(*) FROM weapon_attribution_corrections WHERE undone_at IS NOT NULL"
        )
        .await,
        1,
        "the correction stays as provenance"
    );
    assert_eq!(h.announced.load(Ordering::SeqCst), 2);
    // Undoing twice is refused.
    assert!(matches!(
        h.service.undo(&correction.id).await,
        Err(WeaponReviewError::Conflict(_))
    ));
    // And the shot can be assigned again.
    h.service.assign("s-u1", CANNON).await.unwrap();
}

#[tokio::test]
async fn a_countered_and_a_dangling_shot_price_where_their_cost_lives() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    // The countered shot carries no damage.
    h.service.assign("s-u2", PISTOL).await.unwrap();
    let (phases, cost) = kill_state(&h.db, "s-k").await;
    assert!(phases.contains(&("Pistol".to_string(), 3, 14.0, 0, 0.05)));
    assert!(phases.contains(&("Unknown".to_string(), 1, 25.0, 1, 0.0)));
    assert!(close(cost, 0.35));

    // A shot after the last kill is the session's dangling cost.
    let correction = h.service.assign("s-u3", CANNON).await.unwrap();
    assert!(close(
        scalar_f64(
            &h.db,
            "SELECT dangling_cost FROM tracking_sessions WHERE id = 's'"
        )
        .await,
        0.2
    ));
    h.service.undo(&correction.id).await.unwrap();
    assert!(close(
        scalar_f64(
            &h.db,
            "SELECT dangling_cost FROM tracking_sessions WHERE id = 's'"
        )
        .await,
        0.0
    ));
}

#[tokio::test]
async fn assignments_are_refused_where_they_would_not_be_honest() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    seed_session(&h.db, "live", true).await;
    let refusal = |result: Result<WeaponCorrection, WeaponReviewError>| match result {
        Err(error) => error.to_string(),
        Ok(_) => "assigned".to_string(),
    };
    assert_eq!(
        refusal(h.service.assign("live-u1", CANNON).await),
        "Stop the session before assigning its shots"
    );
    assert_eq!(
        refusal(h.service.assign("s-e1", PISTOL).await),
        "Only a shot no weapon explained can be assigned"
    );
    assert_eq!(
        refusal(h.service.assign("s-u1", RIFLE).await),
        "Only a weapon carried when the shot landed can be assigned"
    );
    assert_eq!(
        refusal(h.service.assign("s-u1", FAP).await),
        "Only a weapon carried when the shot landed can be assigned"
    );
    assert_eq!(
        refusal(h.service.assign("s-u1", SOLD_RIFLE).await),
        "That weapon is no longer in Equipment"
    );
    assert_eq!(
        refusal(h.service.assign("nope", CANNON).await),
        "Shot not found"
    );
    h.service.assign("s-u1", CANNON).await.unwrap();
    assert_eq!(
        refusal(h.service.assign("s-u1", PISTOL).await),
        "This shot is already priced"
    );
    assert!(matches!(
        h.service.undo("nope").await,
        Err(WeaponReviewError::NotFound(_))
    ));
    // Only committed corrections are announced.
    assert_eq!(h.announced.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_shot_missing_from_its_kill_is_refused_and_nothing_moves() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    h.db.with_writer(|conn| {
        conn.execute(
            "DELETE FROM kill_tool_stats WHERE kill_id = 's-k' AND tool_name = 'Unknown'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let before = kill_state(&h.db, "s-k").await;
    assert!(matches!(
        h.service.assign("s-u1", CANNON).await,
        Err(WeaponReviewError::Stored(_))
    ));
    assert_eq!(kill_state(&h.db, "s-k").await, before);
    assert_eq!(
        scalar_i64(&h.db, "SELECT COUNT(*) FROM weapon_attribution_corrections").await,
        0
    );
}

#[tokio::test]
async fn review_lists_shots_by_group_with_what_the_tracker_knew() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    seed_session(&h.db, "live", true).await;
    let page = h
        .service
        .session_shots("s", ShotGroup::Unresolved, 0, 10)
        .await
        .unwrap();
    assert_eq!(page.total, 3);
    let ids: Vec<&str> = page.shots.iter().map(|shot| shot.id.as_str()).collect();
    assert_eq!(ids, vec!["s-u1", "s-u2", "s-u3"]);
    let first = &page.shots[0];
    assert_eq!(first.amount, Some(25.0));
    assert!(first.critical);
    assert_eq!(first.hotbar_tool.as_deref(), Some("Pistol"));
    assert!(first.correctable);
    assert_eq!(first.candidates.len(), 3);
    assert_eq!(page.shots[1].amount, None);

    let page = h
        .service
        .session_shots("s", ShotGroup::Unresolved, 1, 1)
        .await
        .unwrap();
    assert_eq!(page.shots.len(), 1);
    assert_eq!(page.shots[0].id, "s-u2");

    let evidence = h
        .service
        .session_shots("s", ShotGroup::Evidence, 0, 10)
        .await
        .unwrap();
    assert_eq!(evidence.total, 1);
    assert_eq!(evidence.shots[0].review_decision.as_deref(), Some("kept"));
    assert!(!evidence.shots[0].correctable);

    // A running session's shots are listed but not correctable.
    let live = h
        .service
        .session_shots("live", ShotGroup::Unresolved, 0, 10)
        .await
        .unwrap();
    assert!(live.shots.iter().all(|shot| !shot.correctable));

    // An assigned shot names its live correction and is not assignable.
    let correction = h.service.assign("s-u1", CANNON).await.unwrap();
    let page = h
        .service
        .session_shots("s", ShotGroup::Unresolved, 0, 1)
        .await
        .unwrap();
    assert_eq!(
        page.shots[0].correction_id.as_deref(),
        Some(correction.id.as_str())
    );
    assert_eq!(page.shots[0].tool_name.as_deref(), Some("Cannon"));
    assert!(!page.shots[0].correctable);

    assert!(matches!(
        h.service
            .session_shots("s", ShotGroup::Unresolved, -1, 10)
            .await,
        Err(WeaponReviewError::Invalid(_))
    ));
    assert!(matches!(
        h.service
            .session_shots("s", ShotGroup::Unresolved, 0, 0)
            .await,
        Err(WeaponReviewError::Invalid(_))
    ));
}

#[tokio::test]
async fn unpriced_sessions_answer_in_the_callers_order() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    seed_session(&h.db, "t", false).await;
    let asked = vec!["none".to_string(), "t".to_string(), "s".to_string()];
    assert_eq!(
        h.service.unpriced_sessions(asked).await.unwrap(),
        vec!["t".to_string(), "s".to_string()]
    );
    // Once every unpriced shot of a session is assigned, it drops out.
    for shot in ["t-u1", "t-u2", "t-u3"] {
        h.service.assign(shot, PISTOL).await.unwrap();
    }
    assert_eq!(
        h.service
            .unpriced_sessions(vec!["s".to_string(), "t".to_string()])
            .await
            .unwrap(),
        vec!["s".to_string()]
    );
    assert!(h
        .service
        .unpriced_sessions(Vec::new())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn correction_weapons_offer_the_carried_weapons_still_in_equipment_fitting_first() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    let weapons = h.service.correction_weapons("s-u1").await.unwrap();
    let offered: Vec<(&str, bool)> = weapons
        .iter()
        .map(|weapon| (weapon.name.as_str(), weapon.fits))
        .collect();
    assert_eq!(offered, vec![("Cannon", true), ("Pistol", false)]);
    assert!(close(weapons[0].cost_per_shot_ped, 0.2));
    assert!(close(weapons[1].cost_per_shot_ped, 0.05));
    assert!(matches!(
        h.service.correction_weapons("nope").await,
        Err(WeaponReviewError::NotFound(_))
    ));
}

#[tokio::test]
async fn the_detail_block_reads_tallies_shots_and_decisions() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    let block =
        h.db.with_reader(|conn| session_detail_block(conn, "s"))
            .await
            .unwrap();
    assert_eq!(
        block,
        serde_json::json!({
            "correctable": true,
            "agreed": 2,
            "evidenced": 1,
            "evidenceShots": 1,
            "unresolved": 3,
            "unpriced": 3,
            "assigned": 0,
            "effectTicks": 0,
            "reviews": [{
                "id": "s-r",
                "decision": "kept",
                "hotbarTool": "Pistol",
                "evidenceTool": "Cannon",
                "since": 1400.0,
                "decidedAt": 1450.0,
                "repricedShots": 0,
                "costDelta": 0.0,
            }],
        })
    );
    // A session recorded before tallies were kept, with nothing stored.
    h.db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES ('old', 1, 0)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let block =
        h.db.with_reader(|conn| session_detail_block(conn, "old"))
            .await
            .unwrap();
    assert_eq!(block["agreed"], serde_json::Value::Null);
    assert_eq!(block["unresolved"], 0);
    assert_eq!(block["reviews"], serde_json::json!([]));
}

mod conservation {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]

        /// Any sequence of assignments and undos keeps every kill's cost the
        /// sum of its phases and its shots counted once, and undoing every
        /// live correction restores the session exactly.
        #[test]
        fn assignments_conserve_and_undo_restores(
            steps in proptest::collection::vec((0usize..3, any::<bool>(), any::<bool>()), 1..12),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async {
                let h = harness().await;
                seed_session(&h.db, "s", false).await;
                let initial_kill = kill_state(&h.db, "s-k").await;
                let initial_dangling = scalar_f64(&h.db, "SELECT dangling_cost FROM tracking_sessions WHERE id = 's'").await;
                let shots = ["s-u1", "s-u2", "s-u3"];
                let mut live: Vec<Option<String>> = vec![None, None, None];
                for (index, cannon, undo) in steps {
                    match (&live[index], undo) {
                        (Some(correction), true) => {
                            h.service.undo(correction).await.unwrap();
                            live[index] = None;
                        }
                        (None, false) => {
                            let weapon = if cannon { CANNON } else { PISTOL };
                            let correction = h.service.assign(shots[index], weapon).await.unwrap();
                            live[index] = Some(correction.id);
                        }
                        _ => {}
                    }
                    let (phases, cost) = kill_state(&h.db, "s-k").await;
                    let sum: f64 = phases.iter().map(|(_, n, _, _, c)| *n as f64 * c).sum();
                    prop_assert!(close(sum, cost));
                    let count: i64 = phases.iter().map(|(_, n, _, _, _)| n).sum();
                    prop_assert_eq!(count, 5);
                }
                for correction in live.iter().flatten() {
                    h.service.undo(correction).await.unwrap();
                }
                prop_assert_eq!(kill_state(&h.db, "s-k").await, initial_kill);
                let dangling = scalar_f64(&h.db, "SELECT dangling_cost FROM tracking_sessions WHERE id = 's'").await;
                prop_assert!(close(dangling, initial_dangling));
                Ok(())
            })?;
        }
    }
}
