//! The skill-scan completion path, ported from
//! `backend/services/scan_completion.py`: persists scanned skill
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
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::db::{decoded_f64, DbError};
use crate::scan_drift::summarize_level_drift;

/// Latest calibrated level per skill: `MAX(scanned_at)` with
/// `MAX(id)` as the tiebreaker for rows sharing a timestamp.
pub async fn latest_skill_levels(pool: &SqlitePool) -> Result<Vec<(String, f64)>, DbError> {
    let rows = sqlx::query(
        "WITH latest_ts AS ( \
             SELECT skill_name, MAX(scanned_at) AS ts \
             FROM skill_calibrations \
             GROUP BY skill_name \
         ) \
         SELECT skill_name, level FROM skill_calibrations \
         WHERE id IN ( \
             SELECT MAX(s2.id) FROM skill_calibrations s2 \
             JOIN latest_ts m ON s2.skill_name = m.skill_name AND s2.scanned_at = m.ts \
             GROUP BY s2.skill_name \
         )",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            (
                row.try_get::<String, _>(0).unwrap_or_default(),
                decoded_f64(row, 1),
            )
        })
        .collect())
}

async fn last_skill_scan_time(pool: &SqlitePool) -> Result<Option<f64>, DbError> {
    let row = sqlx::query("SELECT MAX(scanned_at) FROM skill_calibrations WHERE source = 'scan'")
        .fetch_one(pool)
        .await?;
    Ok(row.try_get::<Option<f64>, _>(0)?)
}

async fn has_post_scan_skill_updates(pool: &SqlitePool, scan_time: f64) -> Result<bool, DbError> {
    let row = sqlx::query(
        "SELECT 1 FROM skill_calibrations WHERE scanned_at > ? AND source != 'scan' LIMIT 1",
    )
    .bind(scan_time)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// The drift summary the original logs before recalibration: None
/// when no prior scan anchors exist, nothing moved since the last
/// scan, or the comparison itself is empty.
pub async fn scan_drift_summary(
    pool: &SqlitePool,
    scanned_levels: &[(String, f64)],
) -> Result<Option<Value>, DbError> {
    let Some(last_scan) = last_skill_scan_time(pool).await? else {
        return Ok(None);
    };
    if !has_post_scan_skill_updates(pool, last_scan).await? {
        return Ok(None);
    }
    let tracked: Map<String, Value> = latest_skill_levels(pool)
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
async fn archive_prior_skill_anchors(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
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
    let mut query = sqlx::query(sqlx::AssertSqlSafe(insert));
    for name in skill_names {
        query = query.bind(name);
    }
    query.execute(&mut **tx).await?;
    let delete = format!(
        "DELETE FROM skill_calibrations WHERE source = 'scan' AND skill_name IN ({placeholders})"
    );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(delete));
    for name in skill_names {
        query = query.bind(name);
    }
    query.execute(&mut **tx).await?;
    Ok(())
}

/// The completion write: the drift summary computes first (returned
/// for the caller's observability), then prior scan anchors archive
/// and the new anchors insert at one shared instant, under one
/// commit.
pub async fn complete_skill_scan(
    pool: &SqlitePool,
    levels: &[(String, f64)],
    scan_time: f64,
) -> Result<Option<Value>, DbError> {
    let drift = scan_drift_summary(pool, levels).await?;
    let names: Vec<String> = levels.iter().map(|(name, _)| name.clone()).collect();
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    archive_prior_skill_anchors(&mut tx, &names).await?;
    for (skill_name, level) in levels {
        sqlx::query(
            "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
             VALUES (?, ?, 'scan', ?)",
        )
        .bind(skill_name)
        .bind(level)
        .bind(scan_time)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(drift)
}

/// Last scan instant + unique scanned-skill count, for hydrating the
/// scan actor's resting status at startup.
pub async fn hydrate_skill_scan_state(pool: &SqlitePool) -> Result<(Option<f64>, i64), DbError> {
    let row = sqlx::query(
        "SELECT MAX(scanned_at), COUNT(DISTINCT skill_name) FROM skill_calibrations \
         WHERE source = 'scan'",
    )
    .fetch_one(pool)
    .await?;
    let last_time = row.try_get::<Option<f64>, _>(0)?;
    let count = row.try_get::<i64, _>(1)?;
    Ok((last_time, count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn pool_fixture() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        let pool = db.pool().clone();
        (dir, pool)
    }

    async fn seed(pool: &SqlitePool, name: &str, level: f64, source: &str, at: f64) {
        sqlx::query(
            "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(name)
        .bind(level)
        .bind(source)
        .bind(at)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn latest_levels_pick_newest_with_id_tiebreak() {
        let (_dir, pool) = pool_fixture().await;
        seed(&pool, "Rifle", 100.0, "scan", 50.0).await;
        seed(&pool, "Rifle", 101.0, "chatlog", 60.0).await;
        // Two rows share the newest instant: the higher id wins.
        seed(&pool, "Anatomy", 40.0, "chatlog", 60.0).await;
        seed(&pool, "Anatomy", 41.0, "chatlog", 60.0).await;
        let mut levels = latest_skill_levels(&pool).await.unwrap();
        levels.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            levels,
            vec![("Anatomy".to_string(), 41.0), ("Rifle".to_string(), 101.0)]
        );
    }

    #[tokio::test]
    async fn drift_summary_requires_an_anchor_and_movement() {
        let (_dir, pool) = pool_fixture().await;
        let scanned = vec![("Rifle".to_string(), 102.0)];
        // No prior scan anchor: no drift.
        assert!(scan_drift_summary(&pool, &scanned).await.unwrap().is_none());
        seed(&pool, "Rifle", 100.0, "scan", 50.0).await;
        // No movement since the anchor: no drift.
        assert!(scan_drift_summary(&pool, &scanned).await.unwrap().is_none());
        // A chatlog update after the anchor: the comparison runs.
        seed(&pool, "Rifle", 101.0, "chatlog", 60.0).await;
        let drift = scan_drift_summary(&pool, &scanned).await.unwrap().unwrap();
        assert_eq!(drift["compared_count"], 1);
        assert_eq!(drift["worst_name"], "Rifle");

        // Movement BEFORE the anchor never counts: the gate keys on
        // the real anchor instant, not any earlier epoch.
        let (_dir, fresh) = pool_fixture().await;
        seed(&fresh, "Rifle", 99.0, "chatlog", 40.0).await;
        seed(&fresh, "Rifle", 100.0, "scan", 50.0).await;
        assert!(scan_drift_summary(&fresh, &scanned)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn completion_archives_anchors_and_writes_new_ones() {
        let (_dir, pool) = pool_fixture().await;
        seed(&pool, "Rifle", 100.0, "scan", 50.0).await;
        seed(&pool, "Rifle", 100.5, "chatlog", 55.0).await;
        seed(&pool, "Sweat", 10.0, "scan", 50.0).await;

        let levels = vec![("Rifle".to_string(), 101.0)];
        complete_skill_scan(&pool, &levels, 70.0).await.unwrap();

        // The Rifle scan anchor moved to the archive; the chatlog
        // trail and the untouched Sweat anchor stay live.
        let archived: Vec<(String, f64)> =
            sqlx::query("SELECT skill_name, level FROM skill_calibrations_archive ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap()
                .iter()
                .map(|row| (row.try_get(0).unwrap(), decoded_f64(row, 1)))
                .collect();
        assert_eq!(archived, vec![("Rifle".to_string(), 100.0)]);

        let live: Vec<(String, f64, String)> =
            sqlx::query("SELECT skill_name, level, source FROM skill_calibrations ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap()
                .iter()
                .map(|row| {
                    (
                        row.try_get(0).unwrap(),
                        decoded_f64(row, 1),
                        row.try_get(2).unwrap(),
                    )
                })
                .collect();
        assert_eq!(
            live,
            vec![
                ("Rifle".to_string(), 100.5, "chatlog".to_string()),
                ("Sweat".to_string(), 10.0, "scan".to_string()),
                ("Rifle".to_string(), 101.0, "scan".to_string()),
            ]
        );

        let (last, count) = hydrate_skill_scan_state(&pool).await.unwrap();
        assert_eq!(last, Some(70.0));
        assert_eq!(count, 2, "Sweat and Rifle both carry scan anchors");
    }

    #[tokio::test]
    async fn hydration_on_an_empty_table_reads_idle() {
        let (_dir, pool) = pool_fixture().await;
        let (last, count) = hydrate_skill_scan_state(&pool).await.unwrap();
        assert_eq!(last, None);
        assert_eq!(count, 0);
    }
}
