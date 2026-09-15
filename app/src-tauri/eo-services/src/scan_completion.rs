//! The skill-scan completion path: persists scanned skill
//! levels into `skill_calibrations` and computes the drift summary
//! comparing tracked against scanned values before recalibration.
//! Profession levels derive from skill calibrations on read, so there
//! is no separate profession-scan persistence.
//!
//! The original only logs its drift summary; the port computes and
//! returns it (the completion path's caller discards it for now; a
//! log surface joins with the application shell), so the comparison
//! arithmetic stays live and testable rather than dead code.

use serde_json::{Map, Value};

use crate::db::{Db, DbError};
use crate::scan_drift::summarize_level_drift;

/// Latest calibrated level per skill, believed-current across every
/// source: the shared read behind [`crate::db::latest_skill_levels`]
/// (latest `scanned_at`, id as the tiebreaker only).
pub async fn latest_skill_levels(db: &Db) -> Result<Vec<(String, f64)>, DbError> {
    db.latest_skill_calibrations(None).await
}

async fn last_skill_scan_time(db: &Db) -> Result<Option<f64>, DbError> {
    db.with_reader(|conn| {
        Ok(conn.query_row(
            "SELECT MAX(scanned_at) FROM skill_calibrations WHERE source = 'scan'",
            [],
            |row| row.get::<_, Option<f64>>(0),
        )?)
    })
    .await
}

async fn has_post_scan_skill_updates(db: &Db, scan_time: f64) -> Result<bool, DbError> {
    db.with_reader(move |conn| {
        use rusqlite::OptionalExtension as _;
        let found = conn
            .query_row(
                "SELECT 1 FROM skill_calibrations WHERE scanned_at > ? AND source != 'scan' LIMIT 1",
                rusqlite::params![scan_time],
                |_| Ok(()),
            )
            .optional()?;
        Ok(found.is_some())
    })
    .await
}

/// The drift summary the original logs before recalibration: None
/// when no prior scan anchors exist, nothing moved since the last
/// scan, or the comparison itself is empty.
pub async fn scan_drift_summary(
    db: &Db,
    scanned_levels: &[(String, f64)],
) -> Result<Option<Value>, DbError> {
    let Some(last_scan) = last_skill_scan_time(db).await? else {
        return Ok(None);
    };
    if !has_post_scan_skill_updates(db, last_scan).await? {
        return Ok(None);
    }
    let tracked: Map<String, Value> = latest_skill_levels(db)
        .await?
        .into_iter()
        .map(|(name, level)| (name, Value::from(level)))
        .collect();
    let scanned: Map<String, Value> = scanned_levels
        .iter()
        .map(|(name, level)| (name.clone(), Value::from(*level)))
        .collect();
    Ok(summarize_level_drift(&tracked, &scanned))
}

/// Move existing scan anchors for the given skills into the archive,
/// leaving the believed-current chatlog/codex trail live. Runs inside
/// the caller's scan transaction, before the new anchors insert.
fn archive_prior_skill_anchors(
    conn: &rusqlite::Connection,
    skill_names: &[String],
) -> Result<(), DbError> {
    if skill_names.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; skill_names.len()].join(",");
    let insert = format!(
        "INSERT INTO skill_calibrations_archive \
         (original_id, skill_name, level, source, scanned_at) \
         SELECT id, skill_name, level, source, scanned_at \
         FROM skill_calibrations \
         WHERE source = 'scan' AND skill_name IN ({placeholders})"
    );
    conn.execute(&insert, rusqlite::params_from_iter(skill_names))?;
    let delete = format!(
        "DELETE FROM skill_calibrations WHERE source = 'scan' AND skill_name IN ({placeholders})"
    );
    conn.execute(&delete, rusqlite::params_from_iter(skill_names))?;
    Ok(())
}

/// The completion write: the drift summary computes first (returned
/// for the caller's observability), then prior scan anchors archive
/// and the new anchors insert at one shared instant, under one
/// commit.
pub async fn complete_skill_scan(
    db: &Db,
    levels: &[(String, f64)],
    scan_time: f64,
) -> Result<Option<Value>, DbError> {
    let drift = scan_drift_summary(db, levels).await?;
    let names: Vec<String> = levels.iter().map(|(name, _)| name.clone()).collect();
    let levels = levels.to_vec();
    db.with_writer(move |conn| {
        let tx = conn.transaction()?;
        archive_prior_skill_anchors(&tx, &names)?;
        for (skill_name, level) in &levels {
            tx.execute(
                "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                 VALUES (?, ?, 'scan', ?)",
                rusqlite::params![skill_name, level, scan_time],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
    .await?;
    Ok(drift)
}

/// Last scan instant + unique scanned-skill count, for hydrating the
/// scan actor's resting status at startup.
pub async fn hydrate_skill_scan_state(db: &Db) -> Result<(Option<f64>, i64), DbError> {
    db.with_reader(|conn| {
        Ok(conn.query_row(
            "SELECT MAX(scanned_at), COUNT(DISTINCT skill_name) FROM skill_calibrations \
             WHERE source = 'scan'",
            [],
            |row| Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, i64>(1)?)),
        )?)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db_fixture() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        (dir, db)
    }

    async fn seed(db: &Db, name: &str, level: f64, source: &str, at: f64) {
        let name = name.to_string();
        let source = source.to_string();
        db.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                 VALUES (?, ?, ?, ?)",
                rusqlite::params![name, level, source, at],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn latest_levels_pick_newest_with_id_tiebreak() {
        let (_dir, db) = db_fixture().await;
        seed(&db, "Rifle", 100.0, "scan", 50.0).await;
        seed(&db, "Rifle", 101.0, "chatlog", 60.0).await;
        // Two rows share the newest instant: the higher id wins.
        seed(&db, "Anatomy", 40.0, "chatlog", 60.0).await;
        seed(&db, "Anatomy", 41.0, "chatlog", 60.0).await;
        let mut levels = latest_skill_levels(&db).await.unwrap();
        levels.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            levels,
            vec![("Anatomy".to_string(), 41.0), ("Rifle".to_string(), 101.0)]
        );
    }

    #[tokio::test]
    async fn drift_summary_requires_an_anchor_and_movement() {
        let (_dir, db) = db_fixture().await;
        let scanned = vec![("Rifle".to_string(), 102.0)];
        // No prior scan anchor: no drift.
        assert!(scan_drift_summary(&db, &scanned).await.unwrap().is_none());
        seed(&db, "Rifle", 100.0, "scan", 50.0).await;
        // No movement since the anchor: no drift.
        assert!(scan_drift_summary(&db, &scanned).await.unwrap().is_none());
        // A chatlog update after the anchor: the comparison runs.
        seed(&db, "Rifle", 101.0, "chatlog", 60.0).await;
        let drift = scan_drift_summary(&db, &scanned).await.unwrap().unwrap();
        assert_eq!(drift["compared_count"], 1);
        assert_eq!(drift["worst_name"], "Rifle");

        // Movement BEFORE the anchor never counts: the gate keys on
        // the real anchor instant, not any earlier epoch.
        let (_dir, fresh) = db_fixture().await;
        seed(&fresh, "Rifle", 99.0, "chatlog", 40.0).await;
        seed(&fresh, "Rifle", 100.0, "scan", 50.0).await;
        assert!(scan_drift_summary(&fresh, &scanned)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn completion_archives_anchors_and_writes_new_ones() {
        let (_dir, db) = db_fixture().await;
        seed(&db, "Rifle", 100.0, "scan", 50.0).await;
        seed(&db, "Rifle", 100.5, "chatlog", 55.0).await;
        seed(&db, "Sweat", 10.0, "scan", 50.0).await;

        let levels = vec![("Rifle".to_string(), 101.0)];
        complete_skill_scan(&db, &levels, 70.0).await.unwrap();

        // The Rifle scan anchor moved to the archive; the chatlog
        // trail and the untouched Sweat anchor stay live.
        let archived: Vec<(String, f64)> = db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT skill_name, level FROM skill_calibrations_archive ORDER BY id",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
                })?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await
            .unwrap();
        assert_eq!(archived, vec![("Rifle".to_string(), 100.0)]);

        let live: Vec<(String, f64, String)> = db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT skill_name, level, source FROM skill_calibrations ORDER BY id",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await
            .unwrap();
        assert_eq!(
            live,
            vec![
                ("Rifle".to_string(), 100.5, "chatlog".to_string()),
                ("Sweat".to_string(), 10.0, "scan".to_string()),
                ("Rifle".to_string(), 101.0, "scan".to_string()),
            ]
        );

        let (last, count) = hydrate_skill_scan_state(&db).await.unwrap();
        assert_eq!(last, Some(70.0));
        assert_eq!(count, 2, "Sweat and Rifle both carry scan anchors");
    }

    #[tokio::test]
    async fn hydration_on_an_empty_table_reads_idle() {
        let (_dir, db) = db_fixture().await;
        let (last, count) = hydrate_skill_scan_state(&db).await.unwrap();
        assert_eq!(last, None);
        assert_eq!(count, 0);
    }
}
