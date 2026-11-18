//! Harvesting (tree cutting) handlers: the hand-tool equip, the
//! explicit failed-swing event, and the harvest-event construction
//! shared with the loot router.
//!
//! The swing model: every swing is directly countable (a successful
//! swing arrives as its wood loot group, a failed swing as the
//! explicit "Harvest attempt failed" line), so cost is per-swing tool
//! decay times the counted swings, with no inferred-attempt heuristic.
//! A failed swing decays the tool like a successful one; the success
//! flag is stored per row, so if that assumption is ever corrected the
//! historical costs are recomputable from the recorded facts.

use crate::bus_events::BusEvent;
use crate::harvest_yield::{yield_tier_for_names, HarvestYieldSource, HarvestYieldTier};
use crate::ped::Ped;
use crate::tracking_models::{HarvestEvent, LootItem};

use super::actor::TrackerActor;
use super::providers::{GuardrailTool, HarvestGuardrailTools, TreeSize};
use super::session::ActiveSession;
use super::time::{instant_to_epoch, parse_timestamp_instant, resolve_local};
use super::HarvestTool;

/// A live disagreement between the guardrail's loot evidence and the
/// hotbar-equipped tool. Held on the session until resolved: agreeing
/// evidence or any fresh hotbar equip (a press re-syncs the belief)
/// clears it; the overlay reads it as the wrong-tool cue.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct GuardrailMismatch {
    pub(super) expected_tool: String,
    pub(super) observed_tool: Option<String>,
    pub(super) tree_size: TreeSize,
    pub(super) at_epoch: f64,
}

/// Whether a loot item is harvesting output (the wood taxonomy: per
/// species board types plus the shavings by-product). Used to route a
/// loot group whose swing had no hotbar signal.
pub(super) fn is_harvest_loot_item(name: &str) -> bool {
    name == "Wood Shavings" || name.ends_with(" Board")
}

/// Whether a whole loot group is harvesting output: non-empty and all
/// wood (a harvest bundle never mixes wood with mob loot).
pub(super) fn is_harvest_loot_group(items: &[LootItem]) -> bool {
    !items.is_empty()
        && items
            .iter()
            .all(|item| is_harvest_loot_item(&item.item_name))
}

/// The board-output class a loot group testifies to (a harvest bundle
/// never carries two board types; shavings-only groups testify to
/// nothing).
pub(super) fn tree_size_for_group(items: &[LootItem]) -> Option<TreeSize> {
    yield_tier_for_names(items.iter().map(|item| item.item_name.as_str()))
}

/// The guardrail intent a loot group resolves to: the evidenced board
/// class paired with its configured tool. None when the group carries
/// no board, and equally when the evidenced class has no configured
/// tool: that class is outside the guardrail's remit, so neither the
/// attribution override nor the retro pass may act on it.
pub(super) fn guardrail_intent<'a>(
    guardrail: Option<&'a HarvestGuardrailTools>,
    items: &[LootItem],
) -> Option<(TreeSize, &'a GuardrailTool)> {
    guardrail
        .zip(tree_size_for_group(items))
        .and_then(|(tools, size)| tools.for_size(size).map(|tool| (size, tool)))
}

impl TrackerActor {
    /// Handle a hotbar harvesting-tool equip: the tool becomes the
    /// hand item and its per-use cost prices subsequent swings. Like
    /// the heal tool, the equipped tool is hotbar-equipment state that
    /// outlives the session; the re-hydrate nudge fires only against
    /// an active session.
    pub(super) fn on_harvest_tool_changed(&mut self, event: &BusEvent) {
        let BusEvent::ActiveHarvestToolChanged(payload) = event else {
            return;
        };
        if payload.tool_name.is_empty() {
            return;
        }
        let changed = self
            .harvest_tool
            .as_ref()
            .map(|tool| tool.name != payload.tool_name)
            .unwrap_or(true);
        self.harvest_tool = Some(HarvestTool {
            name: payload.tool_name.clone(),
            cost_per_use: Ped(payload.cost_per_use_ped),
        });
        let hand_changed = self.held_item.as_ref().is_none_or(|item| {
            item.0 != payload.tool_name || item.1 != crate::bus_events::HotbarItemKind::Harvesting
        });
        self.held_item = Some((
            payload.tool_name.clone(),
            crate::bus_events::HotbarItemKind::Harvesting,
        ));

        let nudge_session_id = {
            let Some(active) = self.session.active_mut() else {
                return;
            };
            active.harvest_warning_emitted = false;
            // A hotbar press re-syncs the app's belief with the game;
            // any standing wrong-tool cue is resolved by it, and the
            // retro pass may not reach back past this point. Clearing
            // a cue is a readout change, so it nudges even for a
            // re-press of the same tool.
            let cleared_mismatch = active.guardrail_mismatch.take().is_some()
                | active.weapons.attribution.mismatch().is_some();
            active.harvest_press_floor = active.session.harvests.len();
            // The same press re-syncs weapon attribution: no weapon
            // evidence may reach back past it either.
            active.weapons.attribution.resync();
            if changed || hand_changed || cleared_mismatch {
                Some(active.session.id.clone())
            } else {
                None
            }
        };
        if let Some(session_id) = nudge_session_id {
            self.emit_session_event(
                eo_wire::domain_events::TrackingReason::Updated,
                eo_wire::domain_events::TrackingStatus::Active,
                instant_to_epoch(resolve_local(self.clock.now())),
                Some(&session_id),
            );
        }
    }

    /// Handle the explicit failed-swing line: one harvest event with
    /// no loot, costed like a successful swing.
    pub(super) async fn on_harvest_fail(&mut self, event: &BusEvent) {
        let BusEvent::HarvestFail(payload) = event else {
            return;
        };
        let harvest = {
            let Self {
                session,
                harvest_tool,
                harvest_guardrail,
                clock,
                chatlog_clock,
                ..
            } = &mut *self;
            let Some(active) = session.active_mut() else {
                return;
            };
            let now_epoch = parse_timestamp_instant(chatlog_clock, &payload.timestamp)
                .map(instant_to_epoch)
                .unwrap_or_else(|| instant_to_epoch(resolve_local(clock.now())));
            let (tool_name, cost) = Self::no_evidence_swing_cost(
                active,
                harvest_guardrail.as_ref(),
                harvest_tool.as_ref(),
            );
            let (yield_tier, yield_tier_source) = Self::yield_for_no_evidence(
                &active.session.harvests,
                active.harvest_press_floor,
                tool_name.as_deref(),
                now_epoch,
            );
            active.dirty = true;
            let harvest = HarvestEvent {
                id: uuid::Uuid::new_v4().to_string(),
                loot_source_id: None,
                session_id: active.session.id.clone(),
                timestamp: now_epoch,
                success: false,
                tool_name,
                yield_tier,
                yield_tier_source,
                cost_ped: cost,
                loot_total_ped: Ped::ZERO,
                loot_items: Vec::new(),
                context_id: active.intervals.context_id(),
            };
            active.session.harvests.push(harvest.clone());
            harvest
        };
        self.persist_harvest(&harvest).await;
    }

    /// The swing's tool identity and cost under the guardrail: the
    /// board evidence names the output class, the configured intent names
    /// the tool, and the attribution follows the intent even when the
    /// hotbar disagrees (a desynced hotbar belief poisons every swing
    /// until the next press; the evidence is per-swing and unforgeable).
    /// A disagreement is surfaced, never silently recorded: it sets the
    /// session's mismatch state (the overlay cue) and a one-shot
    /// session warning. Swings with no board evidence, and sizes with
    /// no configured intent, fall back to the hotbar belief.
    pub(super) fn guarded_harvest_swing_cost(
        active: &mut ActiveSession,
        guardrail: Option<&HarvestGuardrailTools>,
        harvest_tool: Option<&HarvestTool>,
        items: &[LootItem],
        at_epoch: f64,
    ) -> (Option<String>, Ped) {
        let Some((size, tool)) = guardrail_intent(guardrail, items) else {
            // Board evidence naming a class with no configured tool is
            // outside the guardrail's remit: the hotbar belief stands
            // directly, and a mismatch from a different board class must
            // not leak its tool onto a swing whose own evidence
            // contradicts that size.
            if tree_size_for_group(items).is_some() {
                return Self::harvest_swing_cost(active, harvest_tool);
            }
            return Self::no_evidence_swing_cost(active, guardrail, harvest_tool);
        };
        let observed = harvest_tool.map(|equipped| equipped.name.clone());
        if observed.as_deref() == Some(tool.name.as_str()) {
            active.guardrail_mismatch = None;
        } else {
            if !active.guardrail_warning_emitted {
                active.warnings.push(format!(
                    "Harvest guardrail: {} were looted while {} was equipped; \
                     costs are attributed to {}",
                    size.label(),
                    observed.as_deref().unwrap_or("no tool"),
                    tool.name
                ));
                active.guardrail_warning_emitted = true;
            }
            active.guardrail_mismatch = Some(GuardrailMismatch {
                expected_tool: tool.name.clone(),
                observed_tool: observed,
                tree_size: size,
                at_epoch,
            });
        }
        (Some(tool.name.clone()), Ped(tool.cost_per_use_ped))
    }

    /// The retro pass: when board evidence has just set (or re-armed)
    /// a mismatch, the contiguous run of evidence-less swings
    /// immediately before it was almost surely the same desynced run,
    /// so they are re-stamped to the evidence tool. The walk stops at
    /// any board-bearing swing (its own evidence stands) and at a
    /// chain gap past the retro window (a different tree), and it may
    /// not walk below `floor` (the last hotbar press: swings before it
    /// were stamped under a separately-validated belief). It never
    /// runs on agreeing evidence, so swings the belief was right about
    /// are never rewritten. Returns the rows whose persisted copies
    /// need the same update.
    pub(super) fn restamp_preceding_no_evidence_swings(
        harvests: &mut [HarvestEvent],
        floor: usize,
        evidence_tool: &str,
        evidence_cost: Ped,
        evidence_epoch: f64,
    ) -> Vec<(String, String, Ped)> {
        let mut restamps = Vec::new();
        let mut chain_epoch = evidence_epoch;
        let start = floor.min(harvests.len());
        for harvest in harvests[start..].iter_mut().rev() {
            if harvest.yield_tier_source == Some(HarvestYieldSource::Board) {
                break;
            }
            if chain_epoch - harvest.timestamp > super::GUARDRAIL_RETRO_WINDOW_SECONDS {
                break;
            }
            chain_epoch = harvest.timestamp;
            if harvest.tool_name.as_deref() == Some(evidence_tool) {
                continue;
            }
            harvest.tool_name = Some(evidence_tool.to_string());
            harvest.cost_ped = evidence_cost;
            restamps.push((harvest.id.clone(), evidence_tool.to_string(), evidence_cost));
        }
        restamps
    }

    /// Attribute a boardless swing from the nearest preceding direct
    /// board evidence in the same hotkey regime, tool, and 30-second
    /// window.
    pub(super) fn yield_for_no_evidence(
        harvests: &[HarvestEvent],
        floor: usize,
        tool_name: Option<&str>,
        at_epoch: f64,
    ) -> (HarvestYieldTier, Option<HarvestYieldSource>) {
        let start = floor.min(harvests.len());
        for harvest in harvests[start..].iter().rev() {
            let age = at_epoch - harvest.timestamp;
            if age < 0.0 {
                continue;
            }
            if age > super::HARVEST_YIELD_WINDOW_SECONDS {
                break;
            }
            if harvest.tool_name.as_deref() != tool_name {
                break;
            }
            if harvest.yield_tier_source == Some(HarvestYieldSource::Board) {
                return (harvest.yield_tier, Some(HarvestYieldSource::Inferred));
            }
        }
        (HarvestYieldTier::Unknown, None)
    }

    /// Reconcile boardless swings immediately before new direct board
    /// evidence. Each candidate compares its nearest direct evidence on
    /// both temporal sides: agreeing or one-sided evidence infers the
    /// tier, while conflicting evidence leaves it unknown. Direct board
    /// rows are never rewritten.
    pub(super) fn restamp_preceding_yield_tiers(
        harvests: &mut [HarvestEvent],
        floor: usize,
        evidence_tool: Option<&str>,
        evidence_tier: HarvestYieldTier,
        evidence_epoch: f64,
    ) -> Vec<(String, HarvestYieldTier, Option<HarvestYieldSource>)> {
        let start = floor.min(harvests.len());
        let mut desired = Vec::new();

        for index in (start..harvests.len()).rev() {
            let harvest = &harvests[index];
            let age = evidence_epoch - harvest.timestamp;
            if age < 0.0 {
                continue;
            }
            if age > super::HARVEST_YIELD_WINDOW_SECONDS {
                break;
            }
            if harvest.tool_name.as_deref() != evidence_tool {
                break;
            }
            if harvest.yield_tier_source == Some(HarvestYieldSource::Board) {
                break;
            }

            let previous_direct = harvests[start..index].iter().rev().find(|previous| {
                previous.tool_name.as_deref() == evidence_tool
                    && previous.yield_tier_source == Some(HarvestYieldSource::Board)
                    && harvest.timestamp >= previous.timestamp
                    && harvest.timestamp - previous.timestamp <= super::HARVEST_YIELD_WINDOW_SECONDS
            });
            let (tier, source) = match previous_direct {
                Some(previous) if previous.yield_tier != evidence_tier => {
                    (HarvestYieldTier::Unknown, None)
                }
                _ => (evidence_tier, Some(HarvestYieldSource::Inferred)),
            };
            if harvest.yield_tier != tier || harvest.yield_tier_source != source {
                desired.push((index, tier, source));
            }
        }

        let mut restamps = Vec::with_capacity(desired.len());
        for (index, tier, source) in desired {
            let harvest = &mut harvests[index];
            harvest.yield_tier = tier;
            harvest.yield_tier_source = source;
            restamps.push((harvest.id.clone(), tier, source));
        }
        restamps
    }

    /// The tool for a swing with no board evidence: while a guardrail
    /// mismatch stands the run is already indicted, so the swing
    /// follows the mismatch's expected tool rather than the doubted
    /// hotbar belief; otherwise the belief stands (including its
    /// no-tool warning path).
    pub(super) fn no_evidence_swing_cost(
        active: &mut ActiveSession,
        guardrail: Option<&HarvestGuardrailTools>,
        harvest_tool: Option<&HarvestTool>,
    ) -> (Option<String>, Ped) {
        if let (Some(tools), Some(mismatch)) = (guardrail, active.guardrail_mismatch.as_ref()) {
            if let Some(tool) = tools.for_size(mismatch.tree_size) {
                return (Some(tool.name.clone()), Ped(tool.cost_per_use_ped));
            }
        }
        Self::harvest_swing_cost(active, harvest_tool)
    }

    /// The swing's tool identity and cost: the equipped harvesting
    /// tool's per-use cost, or zero with a one-shot session warning
    /// when no harvesting tool has been equipped via the hotbar (never
    /// guess a cost).
    pub(super) fn harvest_swing_cost(
        active: &mut ActiveSession,
        harvest_tool: Option<&HarvestTool>,
    ) -> (Option<String>, Ped) {
        match harvest_tool {
            Some(tool) => (Some(tool.name.clone()), tool.cost_per_use),
            None => {
                if !active.harvest_warning_emitted {
                    active.warnings.push(
                        "Harvesting detected: no harvesting tool equipped via hotbar".to_string(),
                    );
                    active.harvest_warning_emitted = true;
                }
                (None, Ped::ZERO)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wood_taxonomy_matches_boards_and_shavings_only() {
        assert!(is_harvest_loot_item("Wood Shavings"));
        assert!(is_harvest_loot_item("Short Moonleaf Board"));
        assert!(is_harvest_loot_item("Long Kaisenbrandt Board"));
        assert!(!is_harvest_loot_item("Shrapnel"));
        assert!(!is_harvest_loot_item("Animal Muscle Oil"));
        assert!(!is_harvest_loot_item("Boardwalk Trophy"));
    }

    fn item(name: &str) -> LootItem {
        LootItem {
            item_name: name.to_string(),
            quantity: 1,
            value_ped: 0.01,
            is_enhancer_shrapnel: false,
        }
    }

    #[test]
    fn a_group_is_harvest_only_when_every_item_is_wood() {
        assert!(is_harvest_loot_group(&[
            item("Short Moonleaf Board"),
            item("Wood Shavings"),
        ]));
        assert!(is_harvest_loot_group(&[item("Short Moonleaf Board")]));
        assert!(!is_harvest_loot_group(&[
            item("Short Moonleaf Board"),
            item("Shrapnel"),
        ]));
        assert!(!is_harvest_loot_group(&[]));
    }
}
