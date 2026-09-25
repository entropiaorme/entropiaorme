//! Loot-group and global/HoF handlers: dedup fingerprinting, kill
//! construction from the accumulator, and notable-event correlation.

use eo_wire::normalizer::round_half_even;
use rusqlite::OptionalExtension as _;

use crate::bus_events::{BusEvent, GlobalPayload};
use crate::harvest_yield::HarvestYieldSource;
use crate::loot_filter::is_tracked_loot;
use crate::ped::Ped;
use crate::tracking_models::{HarvestEvent, Kill};

use super::actor::TrackerActor;
use super::harvest::{guardrail_intent, is_harvest_loot_group};
use super::time::{instant_to_epoch, parse_timestamp_instant, python_total_seconds, resolve_local};
use super::{GLOBAL_CORRELATION_WINDOW_SECONDS, LOOT_DEDUP_WINDOW_SECONDS};

/// Where one loot group lands: a mob kill, or a harvesting swing.
enum RoutedLoot {
    Kill(Kill, Vec<super::weapon_evidence::ShotEvidence>),
    Harvest(HarvestEvent),
}

enum ReconciledLootSource {
    Kill {
        total: Ped,
        items: Vec<crate::tracking_models::LootItem>,
    },
    Harvest {
        total: Ped,
        items: Vec<crate::tracking_models::LootItem>,
    },
}

impl TrackerActor {
    /// Reconcile one stable loot source from its committed active item rows.
    /// Quest reward confirmation and reversal both use this path, so partial
    /// reclassification and restoration remain exact in the live aggregate.
    pub(super) async fn reconcile_loot_source(
        &mut self,
        source_id: &str,
    ) -> Result<bool, crate::db::DbError> {
        let source_key = source_id.to_string();
        let reconciled = self
            .db
            .with_reader(move |conn| {
                let kill: Option<(String, f64)> = conn
                    .query_row(
                        "SELECT id, loot_total_ped FROM kills WHERE loot_source_id = ?",
                        rusqlite::params![source_key],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                if let Some((kill_id, total)) = kill {
                    let mut stmt = conn.prepare(
                        "SELECT item_name, quantity, value_ped, is_enhancer_shrapnel \
                         FROM kill_loot_items \
                         WHERE kill_id = ? AND deactivated_at IS NULL ORDER BY id",
                    )?;
                    let items = stmt
                        .query_map(rusqlite::params![kill_id], |row| {
                            Ok(crate::tracking_models::LootItem {
                                item_name: row.get(0)?,
                                quantity: row.get(1)?,
                                value_ped: row.get(2)?,
                                is_enhancer_shrapnel: row.get::<_, i64>(3)? != 0,
                            })
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    return Ok(Some(ReconciledLootSource::Kill {
                        total: Ped(total),
                        items,
                    }));
                }
                let harvest: Option<(String, f64)> = conn
                    .query_row(
                        "SELECT id, loot_total_ped FROM harvest_events WHERE loot_source_id = ?",
                        rusqlite::params![source_key],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                let Some((harvest_id, total)) = harvest else {
                    return Ok(None);
                };
                let mut stmt = conn.prepare(
                    "SELECT item_name, quantity, value_ped, is_enhancer_shrapnel \
                     FROM harvest_loot_items \
                     WHERE harvest_id = ? AND deactivated_at IS NULL ORDER BY id",
                )?;
                let items = stmt
                    .query_map(rusqlite::params![harvest_id], |row| {
                        Ok(crate::tracking_models::LootItem {
                            item_name: row.get(0)?,
                            quantity: row.get(1)?,
                            value_ped: row.get(2)?,
                            is_enhancer_shrapnel: row.get::<_, i64>(3)? != 0,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(Some(ReconciledLootSource::Harvest {
                    total: Ped(total),
                    items,
                }))
            })
            .await?;
        let Some(reconciled) = reconciled else {
            return Ok(false);
        };
        let Some(active) = self.session.active_mut() else {
            return Ok(false);
        };
        let changed = match reconciled {
            ReconciledLootSource::Kill { total, items } => {
                let Some(kill) = active
                    .session
                    .kills
                    .iter_mut()
                    .find(|kill| kill.loot_source_id.as_deref() == Some(source_id))
                else {
                    return Ok(false);
                };
                if kill.loot_items == items && kill.loot_total_ped == total {
                    false
                } else {
                    kill.loot_items = items;
                    kill.loot_total_ped = total;
                    true
                }
            }
            ReconciledLootSource::Harvest { total, items } => {
                let Some(harvest) = active
                    .session
                    .harvests
                    .iter_mut()
                    .find(|harvest| harvest.loot_source_id.as_deref() == Some(source_id))
                else {
                    return Ok(false);
                };
                if harvest.loot_items == items && harvest.loot_total_ped == total {
                    false
                } else {
                    harvest.loot_items = items;
                    harvest.loot_total_ped = total;
                    true
                }
            }
        };
        if changed {
            active.dirty = true;
        }
        Ok(changed)
    }

    /// Handle a loot group from chat.log. A wood group (the harvest
    /// taxonomy) records a harvesting swing; anything else creates a
    /// Kill record from the accumulator, which then resets. Either
    /// record is a detached value by the end of the guard block, so
    /// the persisting DB write runs after release.
    pub(super) async fn on_loot(&mut self, event: &BusEvent) {
        let BusEvent::LootGroup(group) = event else {
            return;
        };
        let (routed, restamps, yield_restamps) = {
            let Self {
                session,
                loot_blacklist,
                harvest_tool,
                harvest_guardrail,
                clock,
                chatlog_clock,
                ..
            } = &mut *self;
            let Some(active) = session.active_mut() else {
                return;
            };

            let total_ped = group.total_ped;
            let now = group
                .timestamp
                .as_deref()
                .and_then(|raw| parse_timestamp_instant(chatlog_clock, raw))
                .unwrap_or_else(|| resolve_local(clock.now()));
            let now_epoch = instant_to_epoch(now);

            // Loot deduplication (same fingerprint within 2s window).
            let first_item = group
                .items
                .first()
                .map(|item| item.item_name.clone())
                .unwrap_or_default();
            let fingerprint = (round_half_even(total_ped, 4), group.items.len(), first_item);
            if let Some((last_fingerprint, last_time)) = &active.last_loot {
                if *last_fingerprint == fingerprint
                    && python_total_seconds(now - *last_time) < LOOT_DEDUP_WINDOW_SECONDS
                {
                    return;
                }
            }
            active.last_loot = Some((fingerprint, now));
            // Past the dedup guard a Kill is always recorded, so the
            // readout changes.
            active.dirty = true;

            let mut items = Vec::new();
            for item in &group.items {
                if is_tracked_loot(&item.item_name, loot_blacklist) {
                    items.push(item.clone());
                }
            }
            let filtered_total_ped = Ped(round_half_even(
                items
                    .iter()
                    .filter(|item| !item.is_enhancer_shrapnel)
                    .map(|item| item.value_ped)
                    .sum(),
                4,
            ));

            // A wood group is a harvesting swing, not a kill. The
            // combat accumulator is untouched: pending shots stay
            // pending toward the next kill (or dangling cost).
            // Classification reads the RAW group, not the blacklist-
            // filtered items: a user filter must trim what gets
            // recorded, never flip what kind of event happened.
            if is_harvest_loot_group(&group.items) {
                let (tool_name, cost) = Self::guarded_harvest_swing_cost(
                    active,
                    harvest_guardrail.as_ref(),
                    harvest_tool.as_ref(),
                    &group.items,
                    now_epoch,
                );
                // Board evidence that leaves a mismatch standing has
                // just contradicted the belief the preceding
                // evidence-less swings were stamped from: re-stamp
                // that contiguous run to the evidence tool. Gated on
                // resolved guardrail intent, not mere board presence:
                // an evidenced size with no configured tool was
                // belief-stamped and proves nothing about the run.
                let restamps = match &tool_name {
                    Some(evidence_tool)
                        if active.guardrail_mismatch.is_some()
                            && guardrail_intent(harvest_guardrail.as_ref(), &group.items)
                                .is_some() =>
                    {
                        let floor = active.harvest_press_floor;
                        Self::restamp_preceding_no_evidence_swings(
                            &mut active.session.harvests,
                            floor,
                            evidence_tool,
                            cost,
                            now_epoch,
                        )
                    }
                    _ => Vec::new(),
                };
                let direct_tier = super::harvest::tree_size_for_group(&group.items);
                let (yield_tier, yield_tier_source, yield_restamps) =
                    if let Some(tier) = direct_tier {
                        let restamps = Self::restamp_preceding_yield_tiers(
                            &mut active.session.harvests,
                            active.harvest_press_floor,
                            tool_name.as_deref(),
                            tier,
                            now_epoch,
                        );
                        (tier, Some(HarvestYieldSource::Board), restamps)
                    } else {
                        let (tier, source) = Self::yield_for_no_evidence(
                            &active.session.harvests,
                            active.harvest_press_floor,
                            tool_name.as_deref(),
                            now_epoch,
                        );
                        (tier, source, Vec::new())
                    };
                let harvest = HarvestEvent {
                    id: uuid::Uuid::new_v4().to_string(),
                    loot_source_id: group.source_id.clone(),
                    session_id: active.session.id.clone(),
                    timestamp: now_epoch,
                    success: true,
                    tool_name,
                    yield_tier,
                    yield_tier_source,
                    cost_ped: cost,
                    loot_total_ped: filtered_total_ped,
                    loot_items: items,
                    context_id: active.intervals.context_id(),
                };
                active.session.harvests.push(harvest.clone());
                (RoutedLoot::Harvest(harvest), restamps, yield_restamps)
            } else {
                // Snapshot the mob stamp from the declaration in force.
                // No declaration stamps "Unknown" with no source; the
                // stamp-source discriminant records the provenance so a
                // future detected stamp reads apart from a declared one.
                let stamped = active.stamped_mob_name();
                let mob_stamp_source = stamped
                    .is_some()
                    .then_some(crate::tracker::MobStampSource::Declared);
                let mob_name = stamped.unwrap_or("Unknown").to_string();
                let (mob_species, mob_maturity) = match &active.declared_mob {
                    Some(declared) if !declared.name.is_empty() => {
                        (declared.species.clone(), declared.maturity.clone())
                    }
                    _ => (String::new(), String::new()),
                };

                let session_id = active.session.id.clone();
                // Read before the accumulator borrow: the stamp is what
                // was in force at the moment the kill settled.
                let context_id = active.intervals.context_id();
                let accumulator = &mut active.accumulator;
                let kill = Kill {
                    id: uuid::Uuid::new_v4().to_string(),
                    loot_source_id: group.source_id.clone(),
                    session_id,
                    mob_name,
                    mob_species,
                    mob_maturity,
                    mob_stamp_source,
                    timestamp: now_epoch,
                    shots_fired: accumulator.shots_fired,
                    damage_dealt: accumulator.damage_dealt,
                    damage_taken: accumulator.damage_taken,
                    critical_hits: accumulator.critical_hits,
                    cost_ped: accumulator.weapon_cost(),
                    enhancer_cost: accumulator.enhancer_cost,
                    loot_total_ped: filtered_total_ped,
                    loot_items: items,
                    tool_stats: std::mem::take(&mut accumulator.tool_stats),
                    is_global: false,
                    is_hof: false,
                    context_id,
                };

                // The kill's stored weapon evidence is written with it.
                let evidence = std::mem::take(&mut accumulator.evidence);
                // Reset accumulator for next kill (tool_stats moved into
                // the kill above, exactly the original's shallow copy
                // followed by a fresh dict).
                accumulator.reset();
                // The regime's shots now sit in this kill.
                active.weapons.attribution.settle_kill(&kill.id);

                // Append the finalised kill to the session; the list tail
                // doubles as the original's `_last_kill` alias.
                active.session.kills.push(kill.clone());
                (RoutedLoot::Kill(kill, evidence), Vec::new(), Vec::new())
            }
        };

        // The routed record is a detached value by here; the borrow of
        // the live session ended with the block above.
        match routed {
            RoutedLoot::Kill(kill, evidence) => self.persist_kill(&kill, evidence).await,
            RoutedLoot::Harvest(harvest) => {
                self.persist_harvest(&harvest).await;
                if !restamps.is_empty() {
                    self.persist_harvest_restamps(&restamps).await;
                }
                if !yield_restamps.is_empty() {
                    self.persist_harvest_yield_restamps(&yield_restamps).await;
                }
            }
        }
    }

    /// Handle a global/HoF event from chat.log: tags the most
    /// recently created kill (globals arrive shortly after loot). The
    /// in-memory tag lands under the guard, capturing the values the
    /// DB writes need; the UPDATE/INSERT run after release.
    pub(super) async fn on_global(&mut self, event: &BusEvent) {
        let BusEvent::Global(payload) = event else {
            return;
        };
        let (event_type, player, subject, raw_value, raw_ts) = match payload {
            GlobalPayload::GlobalKill {
                timestamp,
                player,
                creature,
                value,
            } => ("global_kill", player, creature, *value, timestamp),
            GlobalPayload::HofKill {
                timestamp,
                player,
                creature,
                value,
            } => ("hof_kill", player, creature, *value, timestamp),
            GlobalPayload::GlobalItem {
                timestamp,
                player,
                item,
                value,
            } => ("global_item", player, item, *value, timestamp),
            GlobalPayload::HofItem {
                timestamp,
                player,
                item,
                value,
            } => ("hof_item", player, item, *value, timestamp),
        };
        let (session_id, kill_id, target_is_hof, event_type, mob_or_item, value_ped, ts) = {
            let Self {
                session,
                providers,
                clock,
                chatlog_clock,
                ..
            } = &mut *self;
            let Some(active) = session.active_mut() else {
                return;
            };

            // Filter for own player.
            if providers.player_name.is_empty()
                || player.to_lowercase() != providers.player_name.to_lowercase()
            {
                return;
            }

            active.dirty = true;
            let session_id = active.session.id.clone();
            let event_type = event_type.to_string();
            // The original's falsy chain on creature/item: an empty
            // subject falls through to "Unknown".
            let mob_or_item = if subject.is_empty() {
                "Unknown".to_string()
            } else {
                subject.clone()
            };
            let value_ped = raw_value;
            let is_hof = matches!(event_type.as_str(), "hof_kill" | "hof_item");
            let ts = parse_timestamp_instant(chatlog_clock, raw_ts)
                .map(instant_to_epoch)
                .unwrap_or_else(|| instant_to_epoch(resolve_local(clock.now())));

            // Tag the most recently created kill (staleness check:
            // within 5s). The kills tail is the original's
            // `_last_kill` alias.
            let mut kill_id: Option<String> = None;
            let mut target_is_hof = false;
            if let Some(target) = active.session.kills.last_mut() {
                if (ts - target.timestamp).abs() < GLOBAL_CORRELATION_WINDOW_SECONDS {
                    target.is_global = true;
                    if is_hof {
                        target.is_hof = true;
                    }
                    kill_id = Some(target.id.clone());
                    target_is_hof = target.is_hof;
                }
            }
            (
                session_id,
                kill_id,
                target_is_hof,
                event_type,
                mob_or_item,
                value_ped,
                ts,
            )
        };

        let result = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                if let Some(kill_id) = &kill_id {
                    tx.execute(
                        "UPDATE kills SET is_global = 1, is_hof = ? WHERE id = ?",
                        rusqlite::params![i64::from(target_is_hof), kill_id],
                    )?;
                }
                tx.execute(
                    "INSERT INTO notable_events \
                     (session_id, kill_id, event_type, mob_or_item, value_ped, timestamp) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                    rusqlite::params![session_id, kill_id, event_type, mob_or_item, value_ped, ts],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await;
        // A persistence failure is contained like the original's
        // handler exception: the in-memory tag stands.
        let _ = result;
    }
}
