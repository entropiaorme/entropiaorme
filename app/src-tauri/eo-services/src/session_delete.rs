//! Deleting a recorded session: every row that feeds the statistics from
//! it goes, and every other domain lets go of it (its account readings stay,
//! untagged; its measure and measured costs go with it). Kept apart from the
//! session reads, which may not reach the account module.

use crate::db::Db;
use crate::tracking_reads::EditError;

/// Delete a tracking session and every row that feeds the statistics from
/// it. An active session cannot be deleted (its writes are still in
/// flight); a missing one is a not-found. The cascade runs in one
/// transaction, child-scoped rows first, and repairs the daily rollups for
/// exactly the keys the session touched (captured before the deletes empty
/// those tables). Quest completions, protection cost allocations, stock
/// movements, and the session's interval and context records are kept:
/// they are records of their own surfaces, not statistics of the session.
pub async fn delete_session_impl(db: &Db, session_id: &str) -> Result<(), EditError> {
    // The domain outcome of the writer-core transaction: the guard branches the
    // original raised as edit errors, or the completed cascade.
    enum DeleteOutcome {
        NotFound,
        Active,
        Deleted,
    }

    let sid = session_id.to_string();
    let outcome = db
        .with_writer(move |conn| {
            use rusqlite::OptionalExtension as _;
            let tx = conn.transaction()?;

            // Read the existence-and-active guard inside the transaction, so the
            // check and the cascade are one atomic unit (a separate acquisition
            // for the check could interleave with another write between the read
            // and the delete).
            let is_active: Option<i64> = tx
                .query_row(
                    "SELECT is_active FROM tracking_sessions WHERE id = ?",
                    rusqlite::params![sid],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            let Some(is_active) = is_active else {
                return Ok(DeleteOutcome::NotFound);
            };
            if is_active != 0 {
                return Ok(DeleteOutcome::Active);
            }

            // Capture the keys this session touches before its rows are
            // deleted, so the rollup repair below recomputes exactly those
            // days (harvest days and derived ledger dates included).
            let days = crate::daily_rollup::session_day_keys(&tx, &sid)?;

            // Child-scoped rows first (the kill-scoped tables have no ON DELETE
            // cascade), then the kills, the session-scoped rows, and the session.
            // every-kill: write (a delete reaches every row).
            tx.execute(
                "DELETE FROM kill_tool_stats \
                 WHERE kill_id IN (SELECT id FROM kills WHERE session_id = ?)",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM kill_loot_items \
                 WHERE kill_id IN (SELECT id FROM kills WHERE session_id = ?)",
                rusqlite::params![sid],
            )?;
            // The session's enhancer breaks: those its kills carry and
            // those it carries itself.
            tx.execute(
                "DELETE FROM enhancer_breaks WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM kill_mob_detections WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM kill_mob_carried WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM kill_mob_traces WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM screen_look_minutes WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            // every-kill: write (a delete reaches every row).
            tx.execute(
                "DELETE FROM kills WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM skill_gains WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            // Harvest events and their loot feed the same aggregates as
            // kills (the daily rollup sums them by timestamp), so they leave
            // with the session.
            tx.execute(
                "DELETE FROM harvest_loot_items \
                 WHERE harvest_id IN (SELECT id FROM harvest_events WHERE session_id = ?)",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM harvest_events WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            // Ledger entries computed from this session's events (the
            // enhancer rebate) describe nothing once those events are gone.
            tx.execute(
                "DELETE FROM ledger_entries WHERE source_session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM notable_events WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM session_summaries WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            // SQLite foreign keys are deliberately disabled for this schema,
            // so healing evidence must participate in the explicit cascade.
            // An effect window outlives the session that paid for it, so
            // another session can hold ticks of this session's activations,
            // or heals this session's corrections moved. Those heals first
            // return to what they were before such a correction, then lose
            // their explanation by a deleted activation, now and in the
            // state any later undo would restore: nothing may point at a
            // deleted row.
            tx.execute(
                "UPDATE healing_outputs SET \
                     classification = COALESCE(prior_classification, classification), \
                     activation_id = prior_activation_id, \
                     effect_window_id = prior_effect_window_id, \
                     reason = COALESCE(prior_reason, reason), \
                     correction_id = NULL, prior_classification = NULL, \
                     prior_activation_id = NULL, prior_effect_window_id = NULL, \
                     prior_reason = NULL \
                 WHERE session_id <> ?1 AND correction_id IN \
                     (SELECT id FROM healing_corrections WHERE session_id = ?1)",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "UPDATE healing_outputs SET \
                     activation_id = NULL, effect_window_id = NULL, \
                     classification = CASE WHEN classification = 'effect' \
                         THEN 'unattributed' ELSE classification END, \
                     reason = 'its paying session was deleted' \
                 WHERE session_id <> ?1 AND activation_id IN \
                     (SELECT id FROM healing_activations WHERE session_id = ?1)",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "UPDATE healing_outputs SET \
                     prior_activation_id = NULL, prior_effect_window_id = NULL, \
                     prior_classification = CASE WHEN prior_classification = 'effect' \
                         THEN 'unattributed' ELSE prior_classification END, \
                     prior_reason = 'its paying session was deleted' \
                 WHERE session_id <> ?1 AND prior_activation_id IN \
                     (SELECT id FROM healing_activations WHERE session_id = ?1)",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM healing_corrections WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            // A weapon's damage-over-time effect outlives the session that
            // paid for it too: another session's ticks (and corrections
            // marking a hit as a tick) may name it. They keep standing as
            // ticks, since the hit that paid for them was paid, but lose
            // their pointer to the deleted effect.
            tx.execute(
                "UPDATE weapon_shot_evidence SET effect_window_id = NULL, \
                     reason = 'its paying session was deleted' \
                 WHERE session_id <> ?1 AND effect_window_id IN \
                     (SELECT id FROM weapon_effect_windows WHERE session_id = ?1)",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "UPDATE weapon_attribution_corrections SET effect_window_id = NULL \
                 WHERE session_id <> ?1 AND effect_window_id IN \
                     (SELECT id FROM weapon_effect_windows WHERE session_id = ?1)",
                rusqlite::params![sid],
            )?;
            // Weapon evidence is otherwise scoped to its own session.
            for table in [
                "weapon_attribution_corrections",
                "weapon_shot_evidence",
                "weapon_attribution_reviews",
                "weapon_effect_windows",
            ] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE session_id = ?"),
                    rusqlite::params![sid],
                )?;
            }
            tx.execute(
                "DELETE FROM healing_outputs WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM healing_effect_windows WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM healing_activations WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            crate::consumables::detach_session(&tx, &sid)?;
            // The account readings taken during the session stay (later
            // readings may be stored as changes against them), no longer
            // tagged with it; its measure goes with it.
            crate::account::sessions::release_session(&tx, &sid)?;
            crate::session_costs::forget(&tx, &sid)?;
            // Tools read off the screen during the session describe it alone.
            tx.execute(
                "DELETE FROM tool_sightings WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            // A name set aside only by this session's sightings is set
            // aside by nothing now: it is released.
            crate::tool_review::release_unclaimed_names(&tx)?;
            tx.execute(
                "DELETE FROM tracking_sessions WHERE id = ?",
                rusqlite::params![sid],
            )?;
            crate::session_rollup::drop_session(&tx, &sid)?;

            crate::daily_rollup::refresh_days(&tx, days)?;
            tx.commit()?;
            Ok(DeleteOutcome::Deleted)
        })
        .await?;
    match outcome {
        DeleteOutcome::NotFound => Err(EditError::NotFound("Session not found".to_string())),
        DeleteOutcome::Active => Err(EditError::Conflict(
            "Cannot delete an active session".to_string(),
        )),
        DeleteOutcome::Deleted => Ok(()),
    }
}
