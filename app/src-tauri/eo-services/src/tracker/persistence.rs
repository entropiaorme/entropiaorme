//! The tracker's database writes: kill persistence, the session-end
//! ledger gains, and crash-orphan recovery.

use chrono::{DateTime, Utc};
use eo_wire::normalizer::round_half_even;

use crate::db::DbError;
use crate::ped::Ped;
use crate::session_summary::write_session_summary;
use crate::tracking_models::{HarvestEvent, Kill};

use super::actor::TrackerActor;
use super::time::{epoch_to_instant, local_isoformat};

impl TrackerActor {
    /// Close sessions left open by a crash: end at the latest kill or
    /// healing output (or the start), write the ledger gains and the
    /// summary, and clear the active flag.
    pub(super) async fn recover_orphaned_sessions(&self) -> Result<(), DbError> {
        {
            let rows: Vec<(String, f64)> = self
                .db
                .with_reader(|conn| {
                    let mut stmt = conn.prepare(
                        "SELECT id, started_at FROM tracking_sessions WHERE is_active = 1",
                    )?;
                    let mapped = stmt.query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
                    })?;
                    Ok(mapped.collect::<rusqlite::Result<Vec<_>>>()?)
                })
                .await?;
            for (session_id, started_at) in rows {
                let sid_read = session_id.clone();
                let latest = self
                    .db
                    .with_reader(move |conn| {
                        Ok(conn.query_row(
                            "SELECT MAX(timestamp) FROM (\
                                 SELECT timestamp FROM kills WHERE session_id = ? \
                                 UNION ALL \
                                 SELECT observed_at AS timestamp FROM healing_outputs \
                                 WHERE session_id = ?\
                             )",
                            rusqlite::params![sid_read, sid_read],
                            |row| row.get::<_, Option<f64>>(0),
                        )?)
                    })
                    .await?;
                // The original's falsy fallback, not a None check: a
                // zero maximum also falls back to the session start.
                let ended_at = match latest {
                    Some(latest) if latest != 0.0 => latest,
                    _ => started_at,
                };
                // Each orphan closes atomically: the same one-commit
                // grouping the stop path uses, so a failure mid-recovery
                // leaves that session untouched and still recoverable.
                let sid = session_id.clone();
                self.db
                    .with_writer(move |conn| {
                        let tx = conn.transaction()?;
                        tx.execute(
                            "UPDATE tracking_sessions SET ended_at = ?, is_active = 0 WHERE id = ?",
                            rusqlite::params![ended_at, sid],
                        )?;

                        let end_dt = epoch_to_instant(ended_at);
                        Self::create_enhancer_rebate_ledger_entry(&tx, &sid, end_dt)?;
                        write_session_summary(&tx, &sid)?;
                        crate::daily_rollup::refresh_session_days(&tx, &sid)?;
                        crate::session_rollup::recompute_session(&tx, &sid)?;
                        tx.commit()?;
                        Ok(())
                    })
                    .await?;
            }
            Ok(())
        }
    }
    /// Write a finalised kill to the database: the kill row, its weapon
    /// phases, its loot items, and the stored weapon evidence of its shots,
    /// under one commit.
    pub(super) async fn persist_kill(
        &self,
        kill: &Kill,
        evidence: Vec<super::weapon_evidence::ShotEvidence>,
    ) {
        let kill = kill.clone();
        let result = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT OR REPLACE INTO kills \
                     (id, loot_source_id, session_id, mob_name, mob_species, mob_maturity, \
                      mob_stamp_source, timestamp, shots_fired, damage_dealt, damage_taken, \
                      critical_hits, cost_ped, enhancer_cost, \
                      loot_total_ped, is_global, is_hof, context_id) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        kill.id,
                        kill.loot_source_id,
                        kill.session_id,
                        kill.mob_name,
                        kill.mob_species,
                        kill.mob_maturity,
                        kill.mob_stamp_source.map(|source| source.as_str()),
                        kill.timestamp,
                        kill.shots_fired,
                        kill.damage_dealt,
                        kill.damage_taken,
                        kill.critical_hits,
                        kill.cost_ped.value(),
                        kill.enhancer_cost.value(),
                        kill.loot_total_ped.value(),
                        i64::from(kill.is_global),
                        i64::from(kill.is_hof),
                        kill.context_id,
                    ],
                )?;

                super::weapon_evidence::write_kill_phases(&tx, &kill)?;

                for item in &kill.loot_items {
                    tx.execute(
                        "INSERT INTO kill_loot_items \
                         (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) \
                         VALUES (?, ?, ?, ?, ?)",
                        rusqlite::params![
                            kill.id,
                            item.item_name,
                            item.quantity,
                            item.value_ped,
                            i64::from(item.is_enhancer_shrapnel),
                        ],
                    )?;
                }
                for row in &evidence {
                    row.insert(&tx, Some(&kill.id))?;
                }
                tx.commit()?;
                Ok(())
            })
            .await;
        // Contained like the original's handler exception.
        let _ = result;
    }

    /// Write a harvesting swing to the database: the event row and its
    /// loot items, under one commit, mirroring `persist_kill`.
    pub(super) async fn persist_harvest(&self, harvest: &HarvestEvent) {
        let harvest = harvest.clone();
        let recorded_id = harvest.id.clone();
        let recorded_timestamp = harvest.timestamp;
        let recorded_success = harvest.success;
        let result = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT OR REPLACE INTO harvest_events \
                     (id, loot_source_id, session_id, timestamp, success, tool_name, \
                      yield_tier, yield_tier_source, cost_ped, loot_total_ped, context_id) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        harvest.id,
                        harvest.loot_source_id,
                        harvest.session_id,
                        harvest.timestamp,
                        i64::from(harvest.success),
                        harvest.tool_name,
                        harvest.yield_tier.as_str(),
                        harvest.yield_tier_source.map(|source| source.as_str()),
                        harvest.cost_ped.value(),
                        harvest.loot_total_ped.value(),
                        harvest.context_id,
                    ],
                )?;
                for item in &harvest.loot_items {
                    tx.execute(
                        "INSERT INTO harvest_loot_items \
                         (harvest_id, item_name, quantity, value_ped) \
                         VALUES (?, ?, ?, ?)",
                        rusqlite::params![
                            harvest.id,
                            item.item_name,
                            item.quantity,
                            item.value_ped,
                        ],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await;
        // Contained like the kill path's persistence failure.
        if result.is_ok() {
            use eo_wire::domain_events::{
                HarvestRecorded, HarvestRecordedPayload, HarvestRecordedTag,
            };
            self.bus
                .publish(&crate::bus_events::BusEvent::HarvestRecorded(
                    HarvestRecorded {
                        topic: HarvestRecordedTag,
                        event_version: 1,
                        occurred_at: super::time::to_iso_utc(recorded_timestamp),
                        payload: HarvestRecordedPayload {
                            harvest_id: recorded_id,
                            success: recorded_success,
                        },
                    },
                ));
        }
    }

    /// Update already-persisted swings the guardrail's retro pass
    /// re-stamped: tool identity and cost only, under one commit.
    pub(super) async fn persist_harvest_restamps(&self, restamps: &[(String, String, Ped)]) {
        let restamps = restamps.to_vec();
        let result = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                for (id, tool_name, cost) in &restamps {
                    tx.execute(
                        "UPDATE harvest_events SET tool_name = ?, cost_ped = ? WHERE id = ?",
                        rusqlite::params![tool_name, cost.value(), id],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await;
        // Contained like the other persistence failures.
        let _ = result;
    }

    /// Persist event-level yield inference corrections made when later
    /// direct board evidence closes a bounded action sequence.
    pub(super) async fn persist_harvest_yield_restamps(
        &self,
        restamps: &[(
            String,
            crate::harvest_yield::HarvestYieldTier,
            Option<crate::harvest_yield::HarvestYieldSource>,
        )],
    ) {
        let restamps = restamps.to_vec();
        let result = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                for (id, tier, source) in &restamps {
                    tx.execute(
                        "UPDATE harvest_events SET yield_tier = ?, yield_tier_source = ? \
                         WHERE id = ? AND yield_tier_source IS NOT 'board'",
                        rusqlite::params![tier.as_str(), source.map(|value| value.as_str()), id],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await;
        // Contained like the other persistence failures.
        let _ = result;
    }

    /// Session-end rebate on enhancer-break Shrapnel (full TT value
    /// returned by breaks), recorded as a markup ledger gain.
    pub(super) fn create_enhancer_rebate_ledger_entry(
        conn: &rusqlite::Connection,
        session_id: &str,
        end_time: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let rebate: f64 = conn.query_row(
            "SELECT COALESCE(SUM(kli.value_ped), 0) \
             FROM kill_loot_items kli \
             JOIN kills k ON kli.kill_id = k.id \
             WHERE k.session_id = ? AND COALESCE(kli.is_enhancer_shrapnel, 0) = 1 \
             AND kli.deactivated_at IS NULL",
            rusqlite::params![session_id],
            |row| row.get::<_, f64>(0),
        )?;
        if rebate <= 0.0 {
            return Ok(());
        }
        let date = local_isoformat(end_time);
        conn.execute(
            "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
             VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                date,
                "markup",
                "Enhancer Shrapnel Rebate",
                round_half_even(rebate, 4),
                "enhancer",
            ],
        )?;
        // Same watermark reasoning as the shrapnel-conversion entry:
        // orphan recovery can backdate this day.
        crate::daily_rollup::refresh_days(conn, [date])?;
        Ok(())
    }
}
