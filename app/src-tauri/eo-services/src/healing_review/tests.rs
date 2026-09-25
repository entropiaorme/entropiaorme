//! Healing corrections: each correction's effect, its exact undo, the
//! refusals, the review reads, and conservation over generated sequences.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::*;
use crate::clock::MockClock;

/// A FAP costing 0.03 PED per use, direct 60-100.
const FAP: i64 = 1;
/// A restoration chip costing 0.04 PED per use: a 28-32 direct heal plus
/// 9-11 ticks over 20 seconds.
const RESTORATION: i64 = 2;
/// Not a healing item.
const RIFLE: i64 = 3;

struct Harness {
    _dir: tempfile::TempDir,
    db: Db,
    service: HealingReviewService,
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
    let service = HealingReviewService::new(db.clone(), clock).with_changed(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    db.with_writer(|conn| {
        conn.execute_batch(
            r#"INSERT INTO equipment_library (id, name, item_type, properties_json) VALUES
               (1, 'FAP', 'healing',
                '{"tool_entity": {"economy": {"decay": 3.0}, "min_heal": 60, "max_heal": 100},
                  "markup": 100}'),
               (2, 'Restoration chip', 'healing',
                '{"tool_entity": {"economy": {"decay": 4.0}, "min_heal": 28, "max_heal": 32},
                  "markup": 100,
                  "healing_profile": {"mode": "compound", "direct_min": 28, "direct_max": 32,
                    "effect_duration_seconds": 20, "tick_min": 9, "tick_max": 11,
                    "tick_seconds": 2}}'),
               (3, 'Rifle', 'weapon', '{}');"#,
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

/// One ended session with a FAP use, a restoration use and two of its
/// ticks, three unexplained heals, and a lifesteal heal.
async fn seed_session(db: &Db, session_id: &'static str, active: bool) {
    db.with_writer(move |conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost) \
             VALUES (?1, 1000, ?2, ?3, 0.07)",
            rusqlite::params![session_id, (!active).then_some(2000.0), active as i64],
        )?;
        let id = |name: &str| format!("{session_id}-{name}");
        tx.execute(
            "INSERT INTO healing_activations \
             (id, session_id, equipment_id, tool_name, intent_at, observed_at, chat_timestamp, \
              cost_ped, profile_json, provenance, confirming_output_id) VALUES \
             (?1, ?3, 1, 'FAP', 1100, 1100, 't', 0.03, '{}', 'direct', ?4), \
             (?2, ?3, 2, 'Restoration chip', 1200, 1200, 't', 0.04, '{}', 'direct', ?5)",
            rusqlite::params![id("fap"), id("resto"), session_id, id("o1"), id("o2")],
        )?;
        tx.execute(
            "INSERT INTO healing_effect_windows \
             (id, activation_id, session_id, equipment_id, tool_name, started_at, expires_at, \
              tick_min, tick_max, tick_seconds) \
             VALUES (?1, ?2, ?3, 2, 'Restoration chip', 1200, 1220, 9, 11, 2)",
            rusqlite::params![id("w"), id("resto"), session_id],
        )?;
        for (name, activation, window, at, amount, classification) in [
            ("o1", Some("fap"), None, 1100.0, 80.0, "direct"),
            ("o2", Some("resto"), Some("w"), 1200.0, 30.0, "direct"),
            ("t1", Some("resto"), Some("w"), 1202.0, 10.0, "effect"),
            ("t2", Some("resto"), Some("w"), 1204.0, 10.0, "effect"),
            ("u1", None, None, 1300.0, 80.0, "unattributed"),
            ("u2", None, None, 1302.0, 10.0, "unattributed"),
            ("u3", None, None, 1304.0, 10.0, "unattributed"),
            ("p1", None, None, 1400.0, 5.0, "passive"),
        ] {
            tx.execute(
                "INSERT INTO healing_outputs \
                 (id, session_id, activation_id, effect_window_id, observed_at, chat_timestamp, \
                  amount, classification, reason) VALUES (?1, ?2, ?3, ?4, ?5, 't', ?6, ?7, 'seeded')",
                rusqlite::params![
                    id(name),
                    session_id,
                    activation.map(id),
                    window.map(id),
                    at,
                    amount,
                    classification
                ],
            )?;
        }
        crate::session_summary::write_session_summary(&tx, session_id)?;
        tx.commit()?;
        Ok(())
    })
    .await
    .unwrap();
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

async fn heal_cost(db: &Db, session_id: &str) -> f64 {
    let session_id = session_id.to_string();
    db.with_reader(move |conn| {
        Ok(conn.query_row(
            "SELECT COALESCE(heal_cost, 0) FROM tracking_sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )?)
    })
    .await
    .unwrap()
}

/// The materialised summary's heal cost; a session left with nothing to
/// summarise has no summary and reads as zero, as the list does.
async fn summary_heal_cost(db: &Db, session_id: &str) -> f64 {
    use rusqlite::OptionalExtension;

    let session_id = session_id.to_string();
    db.with_reader(move |conn| {
        Ok(conn
            .query_row(
                "SELECT COALESCE(heal_cost, 0) FROM session_summaries WHERE session_id = ?1",
                [session_id],
                |row| row.get::<_, f64>(0),
            )
            .optional()?
            .unwrap_or(0.0))
    })
    .await
    .unwrap()
}

/// The sum of live activation costs the session carries.
async fn live_activation_cost(db: &Db, session_id: &str) -> f64 {
    let session_id = session_id.to_string();
    db.with_reader(move |conn| {
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(cost_ped), 0) FROM healing_activations \
             WHERE session_id = ?1 AND superseded_at IS NULL",
            [session_id],
            |row| row.get(0),
        )?)
    })
    .await
    .unwrap()
}

/// Every output column plus the originally recorded activations' and
/// windows' live state: what an undo must restore exactly.
async fn evidence(db: &Db, session_id: &str) -> Vec<String> {
    let session_id = session_id.to_string();
    db.with_reader(move |conn| {
        let mut rows = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT id, classification, activation_id, effect_window_id, reason, correction_id, \
                    prior_classification, prior_activation_id, prior_effect_window_id, \
                    prior_reason \
             FROM healing_outputs WHERE session_id = ?1 ORDER BY id",
        )?;
        let outputs = stmt.query_map([&session_id], |row| {
            Ok(format!(
                "output {:?}",
                (
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                )
            ))
        })?;
        rows.extend(outputs.collect::<rusqlite::Result<Vec<_>>>()?);
        let mut stmt = conn.prepare(
            "SELECT id, superseded_at IS NOT NULL FROM healing_activations \
             WHERE session_id = ?1 AND correction_id IS NULL ORDER BY id",
        )?;
        let activations = stmt.query_map([&session_id], |row| {
            Ok(format!(
                "activation {} superseded={}",
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?
            ))
        })?;
        rows.extend(activations.collect::<rusqlite::Result<Vec<_>>>()?);
        let mut stmt = conn.prepare(
            "SELECT w.id, w.superseded_at IS NOT NULL FROM healing_effect_windows w \
             JOIN healing_activations a ON a.id = w.activation_id \
             WHERE w.session_id = ?1 AND a.correction_id IS NULL ORDER BY w.id",
        )?;
        let windows = stmt.query_map([&session_id], |row| {
            Ok(format!(
                "window {} superseded={}",
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?
            ))
        })?;
        rows.extend(windows.collect::<rusqlite::Result<Vec<_>>>()?);
        Ok(rows)
    })
    .await
    .unwrap()
}

async fn output_state(db: &Db, output_id: &str) -> (String, Option<String>, Option<String>) {
    let output_id = output_id.to_string();
    db.with_reader(move |conn| {
        Ok(conn.query_row(
            "SELECT classification, activation_id, correction_id FROM healing_outputs \
             WHERE id = ?1",
            [output_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn not_a_paid_use_takes_the_cost_back_and_unexplains_its_outputs() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;

    let correction = h
        .service
        .correct(CorrectionTarget::NotPaidUse {
            activation_id: "s-resto".into(),
        })
        .await
        .unwrap();
    assert_eq!(correction.kind, CorrectionKind::NotPaidUse);
    assert_eq!(correction.output_id.as_deref(), Some("s-o2"));
    assert!(close(correction.cost_delta_ped, -0.04));
    assert_eq!(h.announced.load(Ordering::SeqCst), 1);

    assert!(close(heal_cost(&h.db, "s").await, 0.03));
    assert!(close(summary_heal_cost(&h.db, "s").await, 0.03));
    for output in ["s-o2", "s-t1", "s-t2"] {
        let (classification, activation, moved_by) = output_state(&h.db, output).await;
        assert_eq!(classification, "unattributed");
        assert_eq!(activation, None);
        assert_eq!(moved_by.as_deref(), Some(correction.id.as_str()));
    }
    // The FAP use and the loose heals are untouched.
    assert_eq!(output_state(&h.db, "s-o1").await.0, "direct");
    assert_eq!(output_state(&h.db, "s-u1").await.2, None);

    let detail =
        h.db.with_reader(|conn| crate::tracking_reads::get_session_read(conn, "s", 0.0))
            .await
            .unwrap()
            .unwrap();
    let healing = &detail["healing"];
    assert_eq!(healing["correctable"], true);
    assert_eq!(healing["activationCount"], 1);
    let resto = healing["activations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "s-resto")
        .expect("a superseded activation stays listed while its correction is live");
    assert_eq!(resto["superseded"], true);
    assert_eq!(resto["correction"]["kind"], "notPaidUse");
    assert_eq!(resto["amount"], 30.0);
}

#[tokio::test]
async fn undoing_a_correction_restores_the_evidence_exactly() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    let before = evidence(&h.db, "s").await;

    let taken_back = h
        .service
        .correct(CorrectionTarget::NotPaidUse {
            activation_id: "s-resto".into(),
        })
        .await
        .unwrap();
    let minted = h
        .service
        .correct(CorrectionTarget::PaidUse {
            output_id: "s-u2".into(),
            equipment_id: RESTORATION,
        })
        .await
        .unwrap();
    assert_ne!(evidence(&h.db, "s").await, before);

    assert_eq!(h.service.undo(&taken_back.id).await.unwrap(), "s");
    h.service.undo(&minted.id).await.unwrap();
    assert_eq!(evidence(&h.db, "s").await, before);
    assert!(close(heal_cost(&h.db, "s").await, 0.07));
    assert!(close(summary_heal_cost(&h.db, "s").await, 0.07));
    assert_eq!(h.announced.load(Ordering::SeqCst), 4);

    let undone: i64 =
        h.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM healing_corrections WHERE undone_at IS NOT NULL",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(undone, 2, "an undone correction stays as provenance");
}

#[tokio::test]
async fn a_paid_use_mints_one_activation_and_claims_the_ticks_its_effect_explains() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;

    let fap = h
        .service
        .correct(CorrectionTarget::PaidUse {
            output_id: "s-u1".into(),
            equipment_id: FAP,
        })
        .await
        .unwrap();
    assert!(close(fap.cost_delta_ped, 0.03));
    assert_eq!(output_state(&h.db, "s-u1").await.0, "direct");

    let resto = h
        .service
        .correct(CorrectionTarget::PaidUse {
            output_id: "s-u2".into(),
            equipment_id: RESTORATION,
        })
        .await
        .unwrap();
    assert!(close(resto.cost_delta_ped, 0.04));
    let (classification, activation, moved_by) = output_state(&h.db, "s-u2").await;
    assert_eq!(classification, "direct");
    assert_eq!(activation.as_deref(), Some(resto.activation_id.as_str()));
    assert_eq!(moved_by.as_deref(), Some(resto.id.as_str()));
    // The later 10-point heal inside the new 20-second effect is its tick.
    let (tick_class, tick_activation, tick_moved_by) = output_state(&h.db, "s-u3").await;
    assert_eq!(tick_class, "effect");
    assert_eq!(
        tick_activation.as_deref(),
        Some(resto.activation_id.as_str())
    );
    assert_eq!(tick_moved_by.as_deref(), Some(resto.id.as_str()));
    // The lifesteal heal already had an explanation and is left alone.
    assert_eq!(output_state(&h.db, "s-p1").await.0, "passive");

    assert!(close(heal_cost(&h.db, "s").await, 0.14));
    assert!(close(live_activation_cost(&h.db, "s").await, 0.14));
    assert!(close(summary_heal_cost(&h.db, "s").await, 0.14));

    let (provenance, context, window_expiry): (String, Option<i64>, f64) =
        h.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT a.provenance, a.context_id, w.expires_at FROM healing_activations a \
                 JOIN healing_effect_windows w ON w.activation_id = a.id WHERE a.id = ?1",
                [&resto.activation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(provenance, "direct");
    assert_eq!(context, None, "the activation takes its output's context");
    assert!(close(window_expiry, 1322.0));
}

#[tokio::test]
async fn refusals_write_nothing_and_announce_nothing() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    seed_session(&h.db, "live", true).await;
    let minted = h
        .service
        .correct(CorrectionTarget::PaidUse {
            output_id: "s-u1".into(),
            equipment_id: FAP,
        })
        .await
        .unwrap();
    let taken_back = h
        .service
        .correct(CorrectionTarget::NotPaidUse {
            activation_id: "s-fap".into(),
        })
        .await
        .unwrap();
    h.service.undo(&taken_back.id).await.unwrap();
    h.service
        .correct(CorrectionTarget::NotPaidUse {
            activation_id: "s-fap".into(),
        })
        .await
        .unwrap();
    let before = evidence(&h.db, "s").await;
    let live_before = evidence(&h.db, "live").await;
    let announced = h.announced.load(Ordering::SeqCst);

    let conflict = |result: Result<HealingCorrection, HealingReviewError>| {
        matches!(result, Err(HealingReviewError::Conflict(_)))
    };
    assert!(conflict(
        h.service
            .correct(CorrectionTarget::NotPaidUse {
                activation_id: "live-fap".into(),
            })
            .await
    ));
    assert!(conflict(
        h.service
            .correct(CorrectionTarget::NotPaidUse {
                activation_id: "s-fap".into(),
            })
            .await
    ));
    assert!(conflict(
        h.service
            .correct(CorrectionTarget::NotPaidUse {
                activation_id: minted.activation_id.clone(),
            })
            .await
    ));
    assert!(conflict(
        h.service
            .correct(CorrectionTarget::PaidUse {
                output_id: "s-o2".into(),
                equipment_id: FAP,
            })
            .await
    ));
    assert!(conflict(
        h.service
            .correct(CorrectionTarget::PaidUse {
                output_id: "s-o1".into(),
                equipment_id: FAP,
            })
            .await
    ));
    assert!(matches!(
        h.service
            .correct(CorrectionTarget::PaidUse {
                output_id: "s-u2".into(),
                equipment_id: RIFLE,
            })
            .await,
        Err(HealingReviewError::Invalid(_))
    ));
    assert!(matches!(
        h.service
            .correct(CorrectionTarget::PaidUse {
                output_id: "s-u2".into(),
                equipment_id: 99,
            })
            .await,
        Err(HealingReviewError::NotFound(_))
    ));
    assert!(matches!(
        h.service
            .correct(CorrectionTarget::NotPaidUse {
                activation_id: "absent".into(),
            })
            .await,
        Err(HealingReviewError::NotFound(_))
    ));
    assert!(matches!(
        h.service.undo(&taken_back.id).await,
        Err(HealingReviewError::Conflict(_))
    ));
    assert!(matches!(
        h.service.undo("absent").await,
        Err(HealingReviewError::NotFound(_))
    ));

    assert_eq!(evidence(&h.db, "s").await, before);
    assert_eq!(evidence(&h.db, "live").await, live_before);
    assert!(close(heal_cost(&h.db, "live").await, 0.07));
    assert_eq!(h.announced.load(Ordering::SeqCst), announced);
}

#[tokio::test]
async fn review_pages_outputs_by_classification_and_offers_fitting_items_first() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    let first = h
        .service
        .session_outputs("s", OutputClassification::Unattributed, 0, 2)
        .await
        .unwrap();
    assert_eq!(first.total, 3);
    let ids: Vec<&str> = first
        .outputs
        .iter()
        .map(|output| output.id.as_str())
        .collect();
    assert_eq!(ids, ["s-u1", "s-u2"]);
    assert!(first.outputs.iter().all(|output| output.correctable));
    let rest = h
        .service
        .session_outputs("s", OutputClassification::Unattributed, 2, 2)
        .await
        .unwrap();
    assert_eq!(rest.outputs.len(), 1);

    let direct = h
        .service
        .session_outputs("s", OutputClassification::Direct, 0, 10)
        .await
        .unwrap();
    assert!(
        direct.outputs.iter().all(|output| !output.correctable),
        "an output that already bills is not offered"
    );
    let ticks = h
        .service
        .session_outputs("s", OutputClassification::Effect, 0, 10)
        .await
        .unwrap();
    assert_eq!(
        ticks.outputs[0].tool_name.as_deref(),
        Some("Restoration chip")
    );
    assert!(
        ticks.outputs[0].correctable,
        "a tick can still be a paid use"
    );
    assert!(matches!(
        h.service
            .session_outputs("s", OutputClassification::Direct, -1, 10)
            .await,
        Err(HealingReviewError::Invalid(_))
    ));

    let for_fap_heal = h.service.correction_tools("s-u1").await.unwrap();
    let names: Vec<(&str, bool)> = for_fap_heal
        .iter()
        .map(|tool| (tool.name.as_str(), tool.fits))
        .collect();
    assert_eq!(names, [("FAP", true), ("Restoration chip", false)]);
    assert!(close(for_fap_heal[0].cost_per_use_ped, 0.03));
    let for_resto_heal = h.service.correction_tools("s-o2").await.unwrap();
    assert_eq!(for_resto_heal[0].name, "Restoration chip");
    assert!(matches!(
        h.service.correction_tools("absent").await,
        Err(HealingReviewError::NotFound(_))
    ));
}

/// A second session holding ticks of the first session's restoration, the
/// way a carried-over effect window records them.
async fn seed_carried_ticks(db: &Db, session_id: &'static str, active: bool) {
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost) \
             VALUES (?1, 1210, ?2, ?3, 0)",
            rusqlite::params![session_id, (!active).then_some(1300.0), active as i64],
        )?;
        for (name, at) in [("tick", 1212.0), ("tick2", 1214.0)] {
            conn.execute(
                "INSERT INTO healing_outputs \
                 (id, session_id, activation_id, effect_window_id, observed_at, chat_timestamp, \
                  amount, classification, reason) \
                 VALUES (?1, ?2, 's-resto', 's-w', ?3, 't', 10, 'effect', 'seeded')",
                rusqlite::params![format!("{session_id}-{name}"), session_id, at],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

async fn set_active(db: &Db, session_id: &'static str, active: bool) {
    db.with_writer(move |conn| {
        conn.execute(
            "UPDATE tracking_sessions SET is_active = ?1 WHERE id = ?2",
            rusqlite::params![active as i64, session_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_correction_waits_for_a_running_session_that_shows_its_effect() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    seed_carried_ticks(&h.db, "t", true).await;
    let before = evidence(&h.db, "t").await;

    assert!(matches!(
        h.service
            .correct(CorrectionTarget::NotPaidUse {
                activation_id: "s-resto".into(),
            })
            .await,
        Err(HealingReviewError::Conflict(_))
    ));
    assert_eq!(evidence(&h.db, "t").await, before);
    assert_eq!(h.announced.load(Ordering::SeqCst), 0);

    // Once that session has stopped, the correction reaches its ticks too.
    set_active(&h.db, "t", false).await;
    let correction = h
        .service
        .correct(CorrectionTarget::NotPaidUse {
            activation_id: "s-resto".into(),
        })
        .await
        .unwrap();
    assert_eq!(output_state(&h.db, "t-tick").await.0, "unattributed");

    set_active(&h.db, "t", true).await;
    assert!(matches!(
        h.service.undo(&correction.id).await,
        Err(HealingReviewError::Conflict(_))
    ));
    set_active(&h.db, "t", false).await;
    h.service.undo(&correction.id).await.unwrap();
    assert_eq!(evidence(&h.db, "t").await, before);
}

#[tokio::test]
async fn deleting_a_session_leaves_no_heal_elsewhere_pointing_at_its_evidence() {
    let h = harness().await;
    seed_session(&h.db, "s", false).await;
    seed_carried_ticks(&h.db, "t", false).await;
    // A correction in the carried session bills its second tick,
    // remembering the use it was a tick of; one in the paying session then
    // takes that use back, moving the first carried tick.
    let paid = h
        .service
        .correct(CorrectionTarget::PaidUse {
            output_id: "t-tick2".into(),
            equipment_id: FAP,
        })
        .await
        .unwrap();
    h.service
        .correct(CorrectionTarget::NotPaidUse {
            activation_id: "s-resto".into(),
        })
        .await
        .unwrap();
    assert_eq!(output_state(&h.db, "t-tick").await.0, "unattributed");

    crate::tracking_reads::delete_session_impl(&h.db, "s")
        .await
        .unwrap();

    let dangling: i64 =
        h.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM healing_outputs o WHERE \
                   (o.activation_id IS NOT NULL AND NOT EXISTS \
                     (SELECT 1 FROM healing_activations a WHERE a.id = o.activation_id)) \
                   OR (o.prior_activation_id IS NOT NULL AND NOT EXISTS \
                     (SELECT 1 FROM healing_activations a WHERE a.id = o.prior_activation_id)) \
                   OR (o.correction_id IS NOT NULL AND NOT EXISTS \
                     (SELECT 1 FROM healing_corrections c WHERE c.id = o.correction_id))",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(dangling, 0);
    // The tick the deleted session's correction had moved is free again.
    assert_eq!(
        output_state(&h.db, "t-tick").await,
        ("unattributed".to_string(), None, None)
    );
    // The billed tick stays billed, and its undo no longer restores a link
    // to a deleted use.
    h.service.undo(&paid.id).await.unwrap();
    assert_eq!(
        output_state(&h.db, "t-tick2").await,
        ("unattributed".to_string(), None, None)
    );
}

mod sequences {
    use proptest::prelude::*;

    use super::*;

    /// One step of a generated review: an index picks among the targets
    /// valid at that moment.
    #[derive(Debug, Clone)]
    enum Step {
        NotPaid(usize),
        Paid(usize, bool),
        Undo(usize),
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            (0usize..8).prop_map(Step::NotPaid),
            (0usize..8, any::<bool>()).prop_map(|(pick, fap)| Step::Paid(pick, fap)),
            (0usize..8).prop_map(Step::Undo),
        ]
    }

    async fn ids(db: &Db, sql: &'static str) -> Vec<String> {
        db.with_reader(move |conn| {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
        .unwrap()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        /// Any sequence of corrections and undos keeps the session's heal
        /// cost equal to its live paid activations and every moved output
        /// restorable; undoing every live correction, in any order, then
        /// restores the evidence, the heal cost, and the summary exactly.
        #[test]
        fn corrections_conserve_cost_and_undo_exactly(
            steps in prop::collection::vec(step(), 1..14),
            unwind_seed in any::<u64>(),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let h = harness().await;
                seed_session(&h.db, "s", false).await;
                let before = evidence(&h.db, "s").await;

                for step in steps {
                    match step {
                        Step::NotPaid(pick) => {
                            let live = ids(&h.db,
                                "SELECT id FROM healing_activations \
                                 WHERE superseded_at IS NULL AND correction_id IS NULL ORDER BY id").await;
                            if !live.is_empty() {
                                h.service.correct(CorrectionTarget::NotPaidUse {
                                    activation_id: live[pick % live.len()].clone(),
                                }).await.unwrap();
                            }
                        }
                        Step::Paid(pick, fap) => {
                            let open = ids(&h.db,
                                "SELECT o.id FROM healing_outputs o WHERE o.correction_id IS NULL \
                                 AND NOT EXISTS (SELECT 1 FROM healing_activations a \
                                   WHERE a.confirming_output_id = o.id AND a.superseded_at IS NULL) \
                                 ORDER BY o.id").await;
                            if !open.is_empty() {
                                h.service.correct(CorrectionTarget::PaidUse {
                                    output_id: open[pick % open.len()].clone(),
                                    equipment_id: if fap { FAP } else { RESTORATION },
                                }).await.unwrap();
                            }
                        }
                        Step::Undo(pick) => {
                            let live = ids(&h.db,
                                "SELECT id FROM healing_corrections WHERE undone_at IS NULL \
                                 ORDER BY id").await;
                            if !live.is_empty() {
                                h.service.undo(&live[pick % live.len()]).await.unwrap();
                            }
                        }
                    }
                    let cost = heal_cost(&h.db, "s").await;
                    assert!(close(cost, live_activation_cost(&h.db, "s").await));
                    assert!(close(cost, summary_heal_cost(&h.db, "s").await));
                    assert!(cost >= 0.0);
                    let unrestorable = ids(&h.db,
                        "SELECT id FROM healing_outputs \
                         WHERE correction_id IS NOT NULL AND prior_classification IS NULL").await;
                    assert!(unrestorable.is_empty());
                }

                let mut live = ids(&h.db,
                    "SELECT id FROM healing_corrections WHERE undone_at IS NULL ORDER BY id").await;
                let mut seed = unwind_seed;
                while !live.is_empty() {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    let next = live.remove((seed >> 33) as usize % live.len());
                    h.service.undo(&next).await.unwrap();
                }
                assert_eq!(evidence(&h.db, "s").await, before);
                assert!(close(heal_cost(&h.db, "s").await, 0.07));
                assert!(close(summary_heal_cost(&h.db, "s").await, 0.07));
            });
        }
    }
}
