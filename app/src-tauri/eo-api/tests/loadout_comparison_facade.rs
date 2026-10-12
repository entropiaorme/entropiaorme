//! Behavioural pins for the loadout comparison over the typed facade:
//! setups built from the catalogue, markups on limited items only, the
//! per-HP curves and the cheapest setup along them, unpriceable setups,
//! and the refusals.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eo_api::loadout_comparison::{
    ComponentPick, LoadoutComparison, SetupComponentRole, SetupPick, UnmodelledReason,
    MAX_COMPARED_SETUPS,
};
use eo_api::{Api, ApiError};
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use serde_json::json;

mod common;

/// A catalogue holding the purchase questions' items.
fn write_snapshot(dir: &Path) {
    let weapons = json!([
        {"id": "bc30", "name": "Exceptional A&P Series Mayhem BC-30, Modified (L)",
         "economy": {"decay": 0.002, "ammo_burn": 804, "efficiency": 80.5, "max_tt": 45},
         "damage": {"impact": 9, "penetration": 29}, "uses_per_minute": 67},
        {"id": "gauss", "name": "Exceptional Kriegerum Mayhem G.2 Gauss Rifle, Perfected (L)",
         "economy": {"decay": 0.01, "ammo_burn": 1636, "efficiency": 91.6, "max_tt": 120},
         "damage": {"impact": 80.8}, "uses_per_minute": 40},
        {"id": "flicker", "name": "Flickerblade Replica, Exceptional (L)",
         "economy": {"decay": 0.02, "ammo_burn": 1850, "efficiency": 88, "max_tt": 250},
         "damage": {"cut": 60, "stab": 18, "impact": 8, "burn": 18}, "uses_per_minute": 45},
        {"id": "scimitar", "name": "Scimitar of the Mages, TWEN Edition",
         "economy": {"decay": 2.336, "ammo_burn": 2950, "efficiency": 88, "max_tt": 500},
         "damage": {"cut": 156}, "uses_per_minute": 42},
        {"id": "mystery", "name": "Mystery Blade",
         "economy": {"decay": 0.5, "ammo_burn": 100}, "damage": {"cut": 20}, "uses_per_minute": 40}
    ]);
    std::fs::write(dir.join("weapons.json"), weapons.to_string()).unwrap();
    std::fs::write(
        dir.join("weapon_amplifiers.json"),
        json!([{"id": "a106", "name": "Omegaton A106",
                "economy": {"decay": 2.902, "ammo_burn": 100, "efficiency": 83.6, "max_tt": 300},
                "damage": {"impact": 18}}])
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("absorbers.json"),
        json!([{"id": "p20", "name": "ArMatrix Extender P20 (L)",
                "economy": {"decay": null, "absorption": 0.2}}])
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("mindforce_implants.json"),
        json!([{"id": "neo", "name": "NeoPsion 85-B Mindforce Implant (L)",
                "economy": {"decay": null, "absorption": 0.2, "max_tt": 188}}])
        .to_string(),
    )
    .unwrap();
    // Profession level is the weighted skill total over 10,000, so a
    // weight of 100 makes a profession one hundredth of its skill.
    let profession = |name: &str, skill: &str| json!({"name": name, "skills": [{"weight": 100, "skill": {"name": skill}}]});
    std::fs::write(
        dir.join("professions.json"),
        json!([
            profession("Animal Looter", "Looting"),
            profession("Mutant Looter", "Looting"),
            profession("Robot Looter", "Looting"),
        ])
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("skills.json"),
        json!([{"name": "Looting"}]).to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("medical_tools.json"),
        json!([{"id": "h1", "name": "Restoration Chip", "economy": {"decay": 1}}]).to_string(),
    )
    .unwrap();
}

async fn api_over(dir: &Path) -> (Api, Db) {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    write_snapshot(&snapshot);
    let data_dir: PathBuf = dir.join("data");
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

/// A scanned Looting skill putting every looter at `looter`.
async fn seed_skills(db: &Db, looter: f64) {
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
             VALUES ('Looting', ?1, 'scan', 1700000000.0)",
            rusqlite::params![looter * 100.0],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

fn pick(catalog_id: &str, markup: Option<f64>) -> ComponentPick {
    ComponentPick {
        catalog_id: catalog_id.to_string(),
        markup_percent: markup,
    }
}

fn weapon_only(catalog_id: &str, markup: Option<f64>) -> SetupPick {
    SetupPick {
        weapon: pick(catalog_id, markup),
        amplifier: None,
        absorber: None,
        implant: None,
    }
}

fn with_p20(catalog_id: &str, markup: f64) -> SetupPick {
    SetupPick {
        absorber: Some(pick("p20", Some(106.0))),
        ..weapon_only(catalog_id, Some(markup))
    }
}

/// Compare over the default 1,000 HP reach at the player's own looter levels.
async fn compare(api: &Api, setups: Vec<SetupPick>) -> Result<LoadoutComparison, ApiError> {
    api.loadout_compare(setups, 1000.0, None).await
}

fn bad_request(result: Result<impl std::fmt::Debug, ApiError>) -> String {
    match result {
        Err(ApiError::BadRequest { message }) => message,
        other => panic!("expected a bad request, got {other:?}"),
    }
}

fn not_found(result: Result<impl std::fmt::Debug, ApiError>) -> String {
    match result {
        Err(ApiError::NotFound { message }) => message,
        other => panic!("expected not found, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setups_compare_per_kill_across_the_whole_curve() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, 50.0).await;
    let comparison = compare(
        &api,
        vec![with_p20("bc30", 1800.0), with_p20("gauss", 1500.0)],
    )
    .await
    .unwrap();

    assert_eq!(comparison.hp_max, 1000.0);
    assert_eq!(comparison.assumptions.model_version, "community_v1");
    assert_eq!(comparison.assumptions.looter_level, 50.0);
    assert_eq!(comparison.assumptions.hit_rate, 0.9);
    let [a, b] = [&comparison.setups[0], &comparison.setups[1]];
    assert_eq!((a.index, b.index), (0, 1));
    assert_eq!(a.name, "Exceptional A&P Series Mayhem BC-30, Modified (L)");
    assert_eq!(a.curve.len(), 1000);
    assert_eq!(a.curve.first().unwrap().hp, 1.0);
    assert_eq!(a.curve.last().unwrap().hp, 1000.0);
    assert!((a.economics.as_ref().unwrap().max_damage - 38.0).abs() < 1e-9);

    // The hand-worked band, read off the curve: 50 to 150 HP, every HP.
    let band = |curve: &[eo_api::loadout_comparison::KillFiguresView]| {
        let points: Vec<_> = curve
            .iter()
            .filter(|point| (50.0..=150.0).contains(&point.hp))
            .collect();
        points.iter().map(|point| point.loss_pec).sum::<f64>() / points.len() as f64
    };
    assert!((band(&a.curve) - 1.84).abs() / 1.84 < 0.04);
    assert!((band(&b.curve) - 1.87).abs() / 1.87 < 0.04);

    // The BC-30 is cheapest below about 95 HP and the Gauss above; under 50
    // HP the Gauss's one-hit kills open a short window of their own.
    let handover = comparison
        .winners
        .iter()
        .find(|segment| segment.index == 1 && (50.0..=150.0).contains(&segment.from_hp))
        .unwrap();
    assert!(
        (handover.from_hp - 99.0).abs() <= 12.0,
        "handover at {}",
        handover.from_hp
    );
    let last = comparison.winners.last().unwrap();
    assert_eq!(last.index, 1);
    assert_eq!(last.to_hp, 1000.0);
    assert_eq!(comparison.winners[0].index, 0);
    assert_eq!(comparison.winners[0].from_hp, 1.0);
    // Consecutive stretches meet and always change hands.
    for pair in comparison.winners.windows(2) {
        assert_eq!(pair[0].to_hp, pair[1].from_hp);
        assert_ne!(pair[0].index, pair[1].index);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn more_than_two_setups_rank_together_in_request_order() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, 41.5).await;
    let comparison = compare(
        &api,
        vec![
            with_p20("flicker", 1200.0),
            weapon_only("scimitar", None),
            with_p20("bc30", 1800.0),
        ],
    )
    .await
    .unwrap();
    let indices: Vec<u32> = comparison.setups.iter().map(|setup| setup.index).collect();
    assert_eq!(indices, [0, 1, 2]);
    assert!(comparison
        .setups
        .iter()
        .all(|setup| setup.curve.len() == 1000));
    // The small-shot BC-30 is cheapest on the smallest mobs; the stretches
    // cover the whole curve.
    assert_eq!(comparison.winners[0].index, 2);
    assert_eq!(comparison.winners[0].from_hp, 1.0);
    assert_eq!(comparison.winners.last().unwrap().to_hp, 1000.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_markup_applies_to_limited_items_and_unlimited_ones_run_at_tt() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, 41.5).await;
    let comparison = compare(
        &api,
        vec![
            weapon_only("scimitar", Some(11_000.0)),
            SetupPick {
                weapon: pick("flicker", None),
                amplifier: Some(pick("a106", Some(140.0))),
                absorber: Some(pick("p20", Some(106.0))),
                implant: Some(pick("neo", Some(110.0))),
            },
        ],
    )
    .await
    .unwrap();
    let scimitar = &comparison.setups[0];
    assert_eq!(scimitar.components[0].markup_percent, 100.0);
    assert!(!scimitar.components[0].is_limited);
    let cost = scimitar.economics.as_ref().unwrap().cost_per_use_pec;
    assert!((cost - (2.336 + 29.5)).abs() < 1e-3, "cost {cost}");

    let flicker = &comparison.setups[1];
    let roles: Vec<SetupComponentRole> = flicker.components.iter().map(|c| c.role).collect();
    assert_eq!(
        roles,
        [
            SetupComponentRole::Weapon,
            SetupComponentRole::Amplifier,
            SetupComponentRole::Absorber,
            SetupComponentRole::Implant
        ]
    );
    // An absent markup on a limited item reads as TT; the unlimited amp's
    // given markup is ignored.
    assert_eq!(flicker.components[0].markup_percent, 100.0);
    assert_eq!(flicker.components[1].markup_percent, 100.0);
    assert_eq!(flicker.components[2].markup_percent, 106.0);
    assert_eq!(flicker.components[3].markup_percent, 110.0);
    let economics = flicker.economics.as_ref().unwrap();
    assert!(
        economics.outside_model_pec > 0.0,
        "absorber and implant decay"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpriceable_setup_says_why_and_leaves_no_ranking() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;
    let comparison = compare(
        &api,
        vec![
            weapon_only("mystery", None),
            weapon_only("bc30", Some(1800.0)),
        ],
    )
    .await
    .unwrap();
    let mystery = &comparison.setups[0];
    assert_eq!(
        *mystery.unmodelled,
        Some(UnmodelledReason::MissingEfficiency)
    );
    assert!(mystery.economics.is_none());
    assert!(mystery.curve.is_empty());
    assert_eq!(comparison.setups[1].curve.len(), 1000);
    // One priced setup has nothing to be cheaper than.
    assert!(comparison.winners.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_comparison_refuses_what_it_cannot_build() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;
    let low = bad_request(
        compare(
            &api,
            vec![weapon_only("bc30", None), weapon_only("gauss", Some(50.0))],
        )
        .await,
    );
    assert_eq!(
        low,
        "Setup 2's weapon markup must be a percentage of at least 100"
    );
    let missing = not_found(
        compare(
            &api,
            vec![
                SetupPick {
                    absorber: Some(pick("nope", Some(100.0))),
                    ..weapon_only("bc30", None)
                },
                weapon_only("gauss", None),
            ],
        )
        .await,
    );
    assert_eq!(missing, "Setup 1's absorber 'nope' is not in the catalogue");
    // A weapon id from another vocabulary is not a weapon.
    not_found(
        compare(
            &api,
            vec![weapon_only("p20", None), weapon_only("gauss", None)],
        )
        .await,
    );
    not_found(
        compare(
            &api,
            vec![weapon_only("", None), weapon_only("gauss", None)],
        )
        .await,
    );
    // A comparison takes one to eight setups.
    bad_request(compare(&api, Vec::new()).await);
    bad_request(
        compare(
            &api,
            vec![weapon_only("bc30", None); MAX_COMPARED_SETUPS + 1],
        )
        .await,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reach_and_looter_level_are_refused_outside_their_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = api_over(dir.path()).await;
    let setups = || vec![with_p20("bc30", 1800.0)];
    for hp_max in [9.0, 100_001.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            bad_request(api.loadout_compare(setups(), hp_max, None).await),
            "a comparison reaches between 10 and 100000 HP",
            "{hp_max}"
        );
    }
    for level in [-1.0, 200.5, f64::NAN] {
        assert_eq!(
            bad_request(api.loadout_compare(setups(), 1000.0, Some(level)).await),
            "a looter level is between 0 and 200",
            "{level}"
        );
    }
    // The ends of both ranges are taken, and the reach is a whole HP.
    let shortest = api
        .loadout_compare(setups(), 10.0, Some(0.0))
        .await
        .unwrap();
    assert_eq!(shortest.hp_max, 10.0);
    assert_eq!(shortest.setups[0].curve.last().unwrap().hp, 10.0);
    assert_eq!(shortest.assumptions.looter_level, 0.0);
    let longer = api
        .loadout_compare(setups(), 4999.6, Some(200.0))
        .await
        .unwrap();
    assert_eq!(longer.hp_max, 5000.0);
    assert_eq!(longer.setups[0].curve.last().unwrap().hp, 5000.0);
    assert_eq!(longer.assumptions.looter_level, 200.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_looter_level_given_stands_in_for_the_players_own() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, 41.5).await;
    let at =
        |level: Option<f64>| api.loadout_compare(vec![with_p20("bc30", 1800.0)], 1000.0, level);

    let own = at(None).await.unwrap();
    assert_eq!(own.assumptions.looter_level, 41.5);
    assert_eq!(own.assumptions.own_looter_level, 41.5);
    let higher = at(Some(80.0)).await.unwrap();
    assert_eq!(higher.assumptions.looter_level, 80.0);
    assert_eq!(higher.assumptions.own_looter_level, 41.5);

    let own_economics = own.setups[0].economics.as_ref().unwrap();
    let higher_economics = higher.setups[0].economics.as_ref().unwrap();
    // A higher looter level narrows the house edge alone; the shortfall
    // moves only by the model's rounding of expected loot to 1e-8 PED.
    assert!(higher_economics.house_edge_pec < own_economics.house_edge_pec);
    assert!(
        (higher_economics.efficiency_shortfall_pec - own_economics.efficiency_shortfall_pec).abs()
            < 1e-5
    );
    assert!(higher_economics.loss_per_use_pec < own_economics.loss_per_use_pec);
    assert!(
        higher.setups[0].curve[500].loss_pec < own.setups[0].curve[500].loss_pec,
        "the curve follows the looter level"
    );
    // At 100 a 100% Efficiency item would lose nothing.
    let top = at(Some(100.0)).await.unwrap();
    assert!(
        top.setups[0]
            .economics
            .as_ref()
            .unwrap()
            .house_edge_pec
            .abs()
            < 1e-9
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn components_carry_their_decay_per_use_and_full_tt() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = api_over(dir.path()).await;
    seed_skills(&db, 50.0).await;
    let comparison = compare(
        &api,
        vec![SetupPick {
            weapon: pick("flicker", Some(1200.0)),
            amplifier: Some(pick("a106", None)),
            absorber: Some(pick("p20", Some(106.0))),
            implant: Some(pick("neo", Some(110.0))),
        }],
    )
    .await
    .unwrap();
    let components = &comparison.setups[0].components;
    let decay: Vec<f64> = components
        .iter()
        .map(|component| component.decay_pec_per_use)
        .collect();
    // The implant takes 20% of the weapon's 0.02 PEC, the absorber 20% of
    // the rest; the amp decays on its own. Markups do not enter TT decay.
    for (actual, expected) in decay.iter().zip([0.0128, 2.902, 0.0032, 0.004]) {
        assert!((actual - expected).abs() < 1e-4, "{decay:?}");
    }
    let max_tt: Vec<Option<f64>> = components
        .iter()
        .map(|component| *component.max_tt_ped)
        .collect();
    assert_eq!(max_tt, [Some(250.0), Some(300.0), None, Some(188.0)]);

    // The loss per use is the premium, the shortfall, the cost outside the
    // model, and the house edge.
    let economics = comparison.setups[0].economics.as_ref().unwrap();
    let parts = economics.consumed_premium_pec
        + economics.efficiency_shortfall_pec
        + economics.outside_model_pec
        + economics.house_edge_pec;
    assert!((parts - economics.loss_per_use_pec).abs() < 1e-9);
}
