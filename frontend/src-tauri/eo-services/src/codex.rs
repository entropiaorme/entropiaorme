//! Codex service, ported from the original Python implementation:
//! species listing, rank breakdowns, claim recording, manual rank
//! calibration, per-rank skill recommendations, and the meta
//! (attribute) codex.
//!
//! Species data comes from the bundled game-data catalogue; player
//! progress and claims live in the application database; claim and
//! calibration timestamps stamp from the injected clock.
//!
//! One original behaviour is reproduced deliberately rather than
//! repaired, so the port stays equivalence-comparable:
//!
//! - **Silent calibration skip.** A claimed reward only lands in
//!   `skill_calibrations` when the skill already has a calibration
//!   history; a first-ever claim for an uncalibrated skill records
//!   the claim and updates progress but writes no calibration row,
//!   so the reward's levels never reach the skill curve.
//!
//! The original's check-then-act claim validation (it read
//! `current_rank` and validated before taking its database lock, so
//! two racing claims for the same rank could both observe it and both
//! record a reward) is NOT reproduced: `claim_rank` advances progress
//! with a conditional upsert gated on the prior rank, so of two racing
//! claims exactly one advances and the loser aborts. In serial use the
//! guard always holds, so single-threaded behaviour (and the
//! cross-language differential) is identical while the race is closed.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};

use crate::clock::Clock;
use crate::codex_categories::{
    build_rank_breakdown, get_category_for_rank, get_rank_cost, get_reward_ped, is_cat4_rank,
    skills_for_category, CAT4_SKILLS,
};
use crate::game_data_store::GameDataStore;
use crate::tracker::naive_to_epoch;
use crate::tt_value_curve::levels_for_tt_value;
use eo_wire::normalizer::round_half_even;

/// The six meta-codex attributes, in sorted order (the original keeps
/// a set and sorts at each use site).
pub const ATTRIBUTES: [&str; 6] = [
    "Agility",
    "Health",
    "Intelligence",
    "Psyche",
    "Stamina",
    "Strength",
];

/// Meta rewards are always 1 PES into an attribute.
pub const META_PED: f64 = 1.0;

/// The service's error surface: `Invalid` carries the original's
/// `ValueError` messages verbatim (HTTP 400 at the router); `Db` is a
/// database failure (HTTP 500).
#[derive(Debug)]
pub enum CodexError {
    Invalid(String),
    Db(sqlx::Error),
}

impl fmt::Display for CodexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodexError::Invalid(message) => write!(f, "{message}"),
            CodexError::Db(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CodexError {}

/// A species' codex parameters from the game-data catalogue.
struct Species {
    base_cost: f64,
    codex_type: Option<String>,
}

/// Codex operations: species listing, rank breakdowns, claim recording.
pub struct CodexService {
    pool: SqlitePool,
    game_data: Arc<GameDataStore>,
    clock: Arc<dyn Clock>,
}

impl CodexService {
    pub fn new(pool: SqlitePool, game_data: Arc<GameDataStore>, clock: Arc<dyn Clock>) -> Self {
        Self {
            pool,
            game_data,
            clock,
        }
    }

    /// All mob species with a codex base cost, cross-referenced with
    /// player rank, sorted rank-descending then name-ascending.
    pub async fn get_all_species(&self) -> Result<Vec<Value>, CodexError> {
        // Deduplicate by species name, first occurrence winning; an
        // entry without a name or base cost is skipped (a skipped
        // no-cost entry does NOT reserve its name, so a later
        // same-name entry with a cost still gets in, exactly as the
        // original's map insertion order works out).
        let mut seen: HashSet<&str> = HashSet::new();
        let mut listed: Vec<(String, f64, Option<String>)> = Vec::new();
        for mob in self.game_data.get_entities("mobs") {
            let Some(species) = species_object(mob) else {
                continue;
            };
            let name = species.get("name").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() || seen.contains(name) {
                continue;
            }
            let Some(base_cost) = base_cost_of(species) else {
                continue;
            };
            seen.insert(name);
            listed.push((
                name.to_string(),
                base_cost,
                species
                    .get("codex_type")
                    .and_then(Value::as_str)
                    .map(String::from),
            ));
        }

        let rows = sqlx::query("SELECT species_name, current_rank FROM codex_progress")
            .fetch_all(&self.pool)
            .await
            .map_err(CodexError::Db)?;
        let rank_map: HashMap<String, i64> = rows
            .into_iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect();

        let mut result: Vec<(i64, String, Value)> = Vec::new();
        for (name, base_cost, codex_type) in listed {
            let rank = rank_map.get(&name).copied().unwrap_or(0);
            let next_rank = if rank < 25 { Some(rank + 1) } else { None };
            // The original gates the derived fields on the next rank's
            // truthiness, so a (hand-edited) rank of -1 yields nextRank
            // 0 with no category or cost.
            let derivable = next_rank.filter(|&next| next != 0);
            let next_category = derivable.map(get_category_for_rank);
            let next_cost =
                derivable.map(|next| round_half_even(get_rank_cost(next, base_cost), 2));
            result.push((
                rank,
                name.clone(),
                json!({
                    "name": name,
                    "baseCost": base_cost,
                    "codexType": codex_type,
                    "currentRank": rank,
                    "nextRank": next_rank,
                    "nextCategory": next_category,
                    "nextCost": next_cost,
                }),
            ));
        }
        result.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        Ok(result.into_iter().map(|(_, _, value)| value).collect())
    }

    /// The 25-rank breakdown for a species, cross-referenced with
    /// claims; `None` when the species is not in the catalogue.
    pub async fn get_species_ranks(&self, species_name: &str) -> Result<Option<Value>, CodexError> {
        let Some(species) = self.find_species(species_name) else {
            return Ok(None);
        };
        let breakdown = build_rank_breakdown(species.base_cost, species.codex_type.as_deref());

        let claims = sqlx::query(
            "SELECT rank, skill_name, ped_value, claimed_at FROM codex_claims \
             WHERE species_name = ? ORDER BY rank",
        )
        .bind(species_name)
        .fetch_all(&self.pool)
        .await
        .map_err(CodexError::Db)?;
        // Built in query order so a duplicate rank's later row wins,
        // as the original's dict comprehension does.
        let mut claims_map: HashMap<i64, (String, f64)> = HashMap::new();
        for row in claims {
            claims_map.insert(row.get(0), (row.get(1), row.get(2)));
        }

        let current_rank = self.current_rank(species_name).await?;

        let ranks: Vec<Value> = breakdown
            .into_iter()
            .map(|item| {
                let claim = claims_map.get(&item.rank);
                let rank = item.rank;
                let mut value = serde_json::to_value(&item).expect("breakdown serialises");
                let entry = value.as_object_mut().expect("breakdown object");
                entry.insert("claimed".into(), json!(claim.is_some()));
                entry.insert(
                    "claimedSkill".into(),
                    json!(claim.map(|(skill, _)| skill.clone())),
                );
                entry.insert("claimedPed".into(), json!(claim.map(|&(_, ped)| ped)));
                entry.insert("isNext".into(), json!(rank == current_rank + 1));
                value
            })
            .collect();

        Ok(Some(json!({
            "speciesName": species_name,
            "baseCost": species.base_cost,
            "codexType": species.codex_type,
            "currentRank": current_rank,
            "ranks": ranks,
        })))
    }

    /// Claim a codex rank reward: validates, records the claim,
    /// advances progress, and updates the skill calibration.
    pub async fn claim_rank(
        &self,
        species_name: &str,
        rank: i64,
        skill_name: &str,
    ) -> Result<Value, CodexError> {
        let species = self.find_species(species_name).ok_or_else(|| {
            CodexError::Invalid(format!(
                "Species '{species_name}' not found in game-data catalogue"
            ))
        })?;

        // A fast-path pre-check for the friendly "expected rank N"
        // error. It is advisory only: the authoritative, race-free rank
        // guard is the conditional progress upsert inside the
        // transaction below, so this read outside the lock cannot admit
        // a double claim.
        let current_rank = self.current_rank(species_name).await?;
        if rank != current_rank + 1 {
            return Err(CodexError::Invalid(format!(
                "Expected rank {}, got {rank}",
                current_rank + 1
            )));
        }
        if rank > 25 {
            return Err(CodexError::Invalid("Maximum rank is 25".to_string()));
        }

        let category = get_category_for_rank(rank);
        let cat4 = is_cat4_rank(rank, species.codex_type.as_deref());

        let in_category = skills_for_category(category)
            .expect("known category")
            .contains(&skill_name);
        let valid = in_category || (cat4 && CAT4_SKILLS.contains(&skill_name));
        if !valid {
            return Err(CodexError::Invalid(format!(
                "Skill '{skill_name}' not valid for rank {rank} (category {category})"
            )));
        }

        // Cat4 skills price through the cat4 divisor (the original
        // checks list membership independently of the cat4 gate; the
        // lists are disjoint, so only a cat4-valid skill reaches it).
        let ped_value = if CAT4_SKILLS.contains(&skill_name) {
            get_reward_ped(rank, species.base_cost, "cat4")
        } else {
            get_reward_ped(rank, species.base_cost, category)
        };

        let now = naive_to_epoch(self.clock.now());

        // One transaction groups the writes. Progress advances FIRST,
        // through a conditional upsert gated on the prior rank, so the
        // check-then-act window is closed: the stored rank equals
        // rank-1 only until the first racer advances it, so of two
        // racing claims for the same rank the upsert fires for exactly
        // one. The loser sees zero rows affected and aborts before any
        // claim or calibration is written. In serial use the guard
        // always holds, so behaviour (and the differential) is
        // unchanged. (For the new-species rank-1 claim there is no row
        // yet, so the plain INSERT path applies and a racing second
        // INSERT conflicts onto the now-false guard.)
        let mut tx = self.pool.begin().await.map_err(CodexError::Db)?;
        let advanced = sqlx::query(
            "INSERT INTO codex_progress (species_name, current_rank, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT(species_name) DO UPDATE SET current_rank = ?, updated_at = ? \
             WHERE codex_progress.current_rank = ? - 1",
        )
        .bind(species_name)
        .bind(rank)
        .bind(now)
        .bind(rank)
        .bind(now)
        .bind(rank)
        .execute(&mut *tx)
        .await
        .map_err(CodexError::Db)?
        .rows_affected();
        if advanced == 0 {
            // Another claim advanced this species' rank between our
            // validation read and this write; abort as the race loser
            // (the transaction rolls back on drop, so nothing lands).
            return Err(CodexError::Invalid(format!(
                "Rank {rank} for '{species_name}' was already claimed"
            )));
        }

        sqlx::query(
            "INSERT INTO codex_claims (species_name, rank, skill_name, ped_value, claimed_at, kind) \
             VALUES (?, ?, ?, ?, ?, 'rank')",
        )
        .bind(species_name)
        .bind(rank)
        .bind(skill_name)
        .bind(ped_value)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(CodexError::Db)?;

        let current_level: Option<f64> = sqlx::query(
            "SELECT level FROM skill_calibrations WHERE skill_name = ? \
             ORDER BY scanned_at DESC LIMIT 1",
        )
        .bind(skill_name)
        .fetch_optional(&mut *tx)
        .await
        .map_err(CodexError::Db)?
        .map(|row| row.get(0));
        if let Some(current_level) = current_level {
            // The reward's TT value buys levels at the current point on
            // the curve. A skill with no calibration history skips this
            // entirely (see the module doc).
            let levels_gained = levels_for_tt_value(current_level, ped_value);
            let new_level = current_level + levels_gained;
            sqlx::query(
                "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                 VALUES (?, ?, 'codex', ?)",
            )
            .bind(skill_name)
            .bind(new_level)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(CodexError::Db)?;
        }
        tx.commit().await.map_err(CodexError::Db)?;

        Ok(json!({
            "speciesName": species_name,
            "rank": rank,
            "skillName": skill_name,
            "pedValue": ped_value,
        }))
    }

    /// Revert the most recent rank claim for a species: step the rank
    /// back one, delete the claim record, and remove the codex-sourced
    /// calibration that claim wrote.
    ///
    /// Only the current rank is cleanly reversible: claims advance
    /// sequentially (the claimable rank is always `current_rank + 1`),
    /// so the latest claim is the one at `current_rank`, and reverting a
    /// lower rank would leave a gap. The reward must have been *claimed*
    /// (not reached by manual `calibrate`) for there to be anything to
    /// undo. The calibration row is matched on the claim instant the two
    /// inserts share, so an uncalibrated-skill claim (which wrote none)
    /// simply removes nothing there.
    ///
    /// A forward feature with no Python-era original; it is mirrored in
    /// the oracle so the OpenAPI contract carries the route, but the
    /// cross-language differential does not drive it.
    pub async fn unclaim_rank(&self, species_name: &str) -> Result<Value, CodexError> {
        let now = naive_to_epoch(self.clock.now());
        let mut tx = self.pool.begin().await.map_err(CodexError::Db)?;

        let current_rank =
            sqlx::query("SELECT current_rank FROM codex_progress WHERE species_name = ?")
                .bind(species_name)
                .fetch_optional(&mut *tx)
                .await
                .map_err(CodexError::Db)?
                .map(|row| row.get::<i64, _>(0))
                .unwrap_or(0);
        if current_rank < 1 {
            return Err(CodexError::Invalid(format!(
                "No claimed rank to unclaim for '{species_name}'"
            )));
        }
        let rank = current_rank;

        let claim = sqlx::query(
            "SELECT skill_name, ped_value, claimed_at FROM codex_claims \
             WHERE species_name = ? AND rank = ? AND kind = 'rank'",
        )
        .bind(species_name)
        .bind(rank)
        .fetch_optional(&mut *tx)
        .await
        .map_err(CodexError::Db)?;
        let Some(claim) = claim else {
            return Err(CodexError::Invalid(format!(
                "Rank {rank} for '{species_name}' was not claimed"
            )));
        };
        let skill_name: String = claim.get(0);
        let ped_value: f64 = claim.get(1);
        let claimed_at: f64 = claim.get(2);

        // Step the rank back, gated on it still being `rank`, so of two
        // racing unclaims exactly one steps it and the loser aborts
        // before deleting anything (the mirror of claim_rank's guard).
        let stepped = sqlx::query(
            "UPDATE codex_progress SET current_rank = ?, updated_at = ? \
             WHERE species_name = ? AND current_rank = ?",
        )
        .bind(rank - 1)
        .bind(now)
        .bind(species_name)
        .bind(rank)
        .execute(&mut *tx)
        .await
        .map_err(CodexError::Db)?
        .rows_affected();
        if stepped == 0 {
            return Err(CodexError::Invalid(format!(
                "Rank {rank} for '{species_name}' was already unclaimed"
            )));
        }

        // Remove the codex-sourced calibration this claim wrote, matched
        // on the instant the claim and calibration inserts share; the
        // id-subquery removes at most one row, and an uncalibrated-skill
        // claim (which wrote none) removes nothing here.
        sqlx::query(
            "DELETE FROM skill_calibrations WHERE id = ( \
                SELECT id FROM skill_calibrations \
                WHERE skill_name = ? AND source = 'codex' AND scanned_at = ? \
                ORDER BY id DESC LIMIT 1)",
        )
        .bind(&skill_name)
        .bind(claimed_at)
        .execute(&mut *tx)
        .await
        .map_err(CodexError::Db)?;

        sqlx::query(
            "DELETE FROM codex_claims WHERE species_name = ? AND rank = ? AND kind = 'rank'",
        )
        .bind(species_name)
        .bind(rank)
        .execute(&mut *tx)
        .await
        .map_err(CodexError::Db)?;

        tx.commit().await.map_err(CodexError::Db)?;

        Ok(json!({
            "speciesName": species_name,
            "rank": rank,
            "skillName": skill_name,
            "pedValue": ped_value,
        }))
    }

    /// Set the codex rank directly, no side effects (manual
    /// calibration).
    pub async fn calibrate(&self, species_name: &str, rank: i64) -> Result<Value, CodexError> {
        if !(0..=25).contains(&rank) {
            return Err(CodexError::Invalid("Rank must be 0-25".to_string()));
        }
        let now = naive_to_epoch(self.clock.now());
        sqlx::query(
            "INSERT INTO codex_progress (species_name, current_rank, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT(species_name) DO UPDATE SET current_rank = ?, updated_at = ?",
        )
        .bind(species_name)
        .bind(rank)
        .bind(now)
        .bind(rank)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(CodexError::Db)?;
        Ok(json!({"speciesName": species_name, "rank": rank}))
    }

    /// Skill choices for a rank, ranked by profession contribution or
    /// HP gain; empty when the species is not in the catalogue.
    ///
    /// Accounts for diminishing returns: a low-weight skill at a low
    /// level can contribute more profession progress than a
    /// high-weight skill at a high level, because the same PED buys
    /// more levels earlier on the TT curve.
    pub async fn get_skill_options(
        &self,
        species_name: &str,
        rank: i64,
        profession: Option<&str>,
        target: &str,
    ) -> Result<Vec<Value>, CodexError> {
        let Some(species) = self.find_species(species_name) else {
            return Ok(Vec::new());
        };

        let category = get_category_for_rank(rank);
        let cat4 = is_cat4_rank(rank, species.codex_type.as_deref());

        let mut skill_entries: Vec<(&'static str, &'static str, f64)> = Vec::new();
        for &skill_name in skills_for_category(category).expect("known category") {
            let ped = get_reward_ped(rank, species.base_cost, category);
            skill_entries.push((skill_name, category, ped));
        }
        if cat4 {
            for &skill_name in CAT4_SKILLS {
                let ped = get_reward_ped(rank, species.base_cost, "cat4");
                skill_entries.push((skill_name, "cat4", ped));
            }
        }

        // Profession weights, when a (non-empty) profession is named:
        // the first matching profession's skill list, weight defaults
        // applied as the original's `or 0`.
        let mut weight_map: HashMap<&str, i64> = HashMap::new();
        if let Some(profession) = profession.filter(|name| !name.is_empty()) {
            for entry in self.game_data.get_entities("professions") {
                if entry.get("name").and_then(Value::as_str) != Some(profession) {
                    continue;
                }
                for skill_entry in entry
                    .get("skills")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                {
                    let name = skill_entry
                        .get("skill")
                        .and_then(|skill| skill.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let weight = skill_entry
                        .get("weight")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    if !name.is_empty() {
                        weight_map.insert(name, weight);
                    }
                }
                break;
            }
        }

        let mut hp_map: HashMap<&str, f64> = HashMap::new();
        for skill in self.game_data.get_entities("skills") {
            let name = skill.get("name").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let hp_increase = skill
                .get("hp_increase")
                .filter(|value| !value.is_null())
                .map(|value| value.as_f64().expect("numeric hp_increase"))
                .unwrap_or(0.0);
            hp_map.insert(name, hp_increase);
        }

        let mut skills: Vec<Value> = Vec::new();
        for (skill_name, cat, ped) in skill_entries {
            let current_level = self.skill_level(skill_name).await?;
            let levels_gained = levels_for_tt_value(current_level.unwrap_or(0.0), ped);
            let weight = weight_map.get(skill_name).copied().unwrap_or(0);
            let prof_contribution = if weight > 0 {
                round_half_even(levels_gained * weight as f64 / 10000.0, 6)
            } else {
                0.0
            };
            let hp_increase = hp_map.get(skill_name).copied().unwrap_or(0.0);
            let hp_gain = if hp_increase > 0.0 {
                round_half_even(levels_gained / hp_increase, 6)
            } else {
                0.0
            };

            skills.push(json!({
                "skillName": skill_name,
                "category": cat,
                "rewardPed": ped,
                "currentLevel": current_level.map(|level| round_half_even(level, 1)),
                "levelsGained": round_half_even(levels_gained, 2),
                "professionWeight": weight,
                "profContribution": prof_contribution,
                "hpIncrease": if hp_increase > 0.0 {
                    json!(round_half_even(hp_increase, 2))
                } else {
                    Value::Null
                },
                "hpGain": hp_gain,
            }));
        }

        // Both orderings sort the rendered (rounded) fields, exactly
        // as the original sorts its dicts; the stable sort preserves
        // entry order on full ties.
        let field = |value: &Value, key: &str| value[key].as_f64().expect("numeric sort field");
        if target == "hp" {
            // Highest HP gain first, then lower current level (absent
            // levels last), then name.
            skills.sort_by(|a, b| {
                field(b, "hpGain")
                    .partial_cmp(&field(a, "hpGain"))
                    .expect("finite hpGain")
                    .then_with(|| {
                        let level =
                            |value: &Value| value["currentLevel"].as_f64().unwrap_or(f64::INFINITY);
                        level(a).partial_cmp(&level(b)).expect("finite level")
                    })
                    .then_with(|| compare_names(a, b))
            });
        } else {
            // Highest profession contribution first, then weight, then
            // name.
            skills.sort_by(|a, b| {
                field(b, "profContribution")
                    .partial_cmp(&field(a, "profContribution"))
                    .expect("finite contribution")
                    .then_with(|| {
                        let weight =
                            |value: &Value| value["professionWeight"].as_i64().unwrap_or(0);
                        weight(b).cmp(&weight(a))
                    })
                    .then_with(|| compare_names(a, b))
            });
        }

        // 1-based rank over the skills relevant to the active target.
        let mut rank_counter = 0i64;
        for skill in &mut skills {
            let relevant = if target == "hp" {
                skill["hpGain"].as_f64().expect("finite hpGain") > 0.0
            } else {
                skill["professionWeight"].as_i64().unwrap_or(0) > 0
            };
            let recommend = if relevant {
                rank_counter += 1;
                json!(rank_counter)
            } else {
                Value::Null
            };
            skill
                .as_object_mut()
                .expect("skill object")
                .insert("recommendRank".into(), recommend);
        }

        Ok(skills)
    }

    /// Claim a meta codex reward: 1 PES into an attribute, persisted
    /// in `codex_claims` with `kind='meta'` and sentinel species and
    /// skill columns (no calibration update; no attribute curve
    /// exists).
    pub async fn meta_claim(&self, attribute_name: &str) -> Result<Value, CodexError> {
        if !ATTRIBUTES.contains(&attribute_name) {
            return Err(CodexError::Invalid(format!(
                "'{attribute_name}' is not an attribute. \
                 Valid: ['Agility', 'Health', 'Intelligence', 'Psyche', 'Stamina', 'Strength']"
            )));
        }
        let now = naive_to_epoch(self.clock.now());
        sqlx::query(
            "INSERT INTO codex_claims \
             (species_name, rank, skill_name, ped_value, claimed_at, kind, attribute_name) \
             VALUES ('__meta__', 0, ?, ?, ?, 'meta', ?)",
        )
        .bind(attribute_name)
        .bind(META_PED)
        .bind(now)
        .bind(attribute_name)
        .execute(&self.pool)
        .await
        .map_err(CodexError::Db)?;
        Ok(json!({
            "attributeName": attribute_name,
            "pedValue": META_PED,
        }))
    }

    /// The six attributes with their current calibrated levels.
    pub async fn get_meta_attributes(&self) -> Result<Vec<Value>, CodexError> {
        let mut result = Vec::with_capacity(ATTRIBUTES.len());
        for attribute in ATTRIBUTES {
            let level = self.skill_level(attribute).await?;
            result.push(json!({
                "name": attribute,
                "currentLevel": level.map(|level| round_half_even(level, 1)),
            }));
        }
        Ok(result)
    }

    /// Species parameters from the catalogue: the FIRST name match
    /// decides, and a first match without a base cost is a miss even
    /// if a later same-name entry carries one (the listing path skips
    /// past such entries instead; both behaviours are the original's).
    fn find_species(&self, species_name: &str) -> Option<Species> {
        for mob in self.game_data.get_entities("mobs") {
            let Some(species) = species_object(mob) else {
                continue;
            };
            if species.get("name").and_then(Value::as_str) != Some(species_name) {
                continue;
            }
            let base_cost = base_cost_of(species)?;
            return Some(Species {
                base_cost,
                codex_type: species
                    .get("codex_type")
                    .and_then(Value::as_str)
                    .map(String::from),
            });
        }
        None
    }

    /// The species' current rank, defaulting to 0 when unranked.
    async fn current_rank(&self, species_name: &str) -> Result<i64, CodexError> {
        Ok(
            sqlx::query("SELECT current_rank FROM codex_progress WHERE species_name = ?")
                .bind(species_name)
                .fetch_optional(&self.pool)
                .await
                .map_err(CodexError::Db)?
                .map(|row| row.get(0))
                .unwrap_or(0),
        )
    }

    /// The latest calibrated level for a skill, by scan instant (no
    /// further tiebreak, as the original; both engines resolve equal
    /// instants identically over the same schema and index).
    async fn skill_level(&self, skill_name: &str) -> Result<Option<f64>, CodexError> {
        Ok(sqlx::query(
            "SELECT level FROM skill_calibrations WHERE skill_name = ? \
             ORDER BY scanned_at DESC LIMIT 1",
        )
        .bind(skill_name)
        .fetch_optional(&self.pool)
        .await
        .map_err(CodexError::Db)?
        .map(|row| row.get(0)))
    }
}

/// The mob's `species` mapping, skipping absent, null, and empty ones
/// (the original's falsiness test over the optional dict).
fn species_object(mob: &Value) -> Option<&serde_json::Map<String, Value>> {
    mob.get("species")
        .and_then(Value::as_object)
        .filter(|object| !object.is_empty())
}

/// The species' codex base cost: absent and null read as missing; a
/// present cost must be numeric (the catalogue emits numbers or null).
fn base_cost_of(species: &serde_json::Map<String, Value>) -> Option<f64> {
    species
        .get("codex_base_cost")
        .filter(|value| !value.is_null())
        .map(|value| value.as_f64().expect("numeric codex base cost"))
}

/// Name-ascending comparison over rendered skill rows.
fn compare_names(a: &Value, b: &Value) -> std::cmp::Ordering {
    a["skillName"]
        .as_str()
        .unwrap_or("")
        .cmp(b["skillName"].as_str().unwrap_or(""))
}

// Expected values in these tests are the original implementation's
// outputs, computed by running the original Python implementation
// over byte-identical catalogue fixtures and database seeds.
#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::NaiveDateTime;

    use super::*;
    use crate::clock::MockClock;
    use crate::db::Db;

    fn start_instant() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-03-01 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap()
    }

    /// The synthetic catalogue: a duplicate species (first wins), a
    /// nameless species, a missing and an empty species object, and a
    /// species whose first entry has no base cost but whose second
    /// does (the listing/lookup divergence pair).
    fn write_snapshot(dir: &Path) {
        std::fs::write(
            dir.join("mobs.json"),
            serde_json::to_string(&json!([
                {"name": "Mob A", "species": {"name": "Boar", "codex_base_cost": 37.5, "codex_type": "Mob"}},
                {"name": "Mob A Variant", "species": {"name": "Boar", "codex_base_cost": 99.0, "codex_type": "Mob"}},
                {"name": "Looter", "species": {"name": "Looter Bird", "codex_base_cost": 10.0, "codex_type": "MobLooter"}},
                {"name": "Nameless", "species": {"name": "", "codex_base_cost": 5.0}},
                {"name": "NoSpecies"},
                {"name": "EmptySpecies", "species": {}},
                {"name": "Costless First", "species": {"name": "Ghost", "codex_base_cost": null}},
                {"name": "Costless Second", "species": {"name": "Ghost", "codex_base_cost": 7.0}},
            ]))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("professions.json"),
            serde_json::to_string(&json!([
                {"name": "Sniper", "skills": [
                    {"skill": {"name": "Rifle"}, "weight": 50},
                    {"skill": {"name": "Aim"}, "weight": 20},
                    {"skill": {"name": "Anatomy"}, "weight": 0},
                    {"skill": {"name": "Zoology"}, "weight": 10},
                    {"skill": null, "weight": 99},
                    {"skill": {"name": ""}, "weight": 7},
                ]},
                {"name": "Sniper", "skills": [{"skill": {"name": "Rifle"}, "weight": 1}]},
            ]))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("skills.json"),
            serde_json::to_string(&json!([
                {"name": "Athletics", "hp_increase": 20},
                {"name": "Rifle", "hp_increase": null},
                {"name": "Aim", "hp_increase": 0},
                {"name": "Dodge", "hp_increase": 12},
                {"name": "Zoology", "hp_increase": 5.5},
                {"name": "Agility", "hp_increase": 10},
            ]))
            .unwrap(),
        )
        .unwrap();
    }

    async fn service(dir: &Path) -> (CodexService, SqlitePool) {
        let snapshot = dir.join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        write_snapshot(&snapshot);
        let db = Db::open(&dir.join("entropia_orme.db")).await.unwrap();
        let pool = db.pool().clone();
        let game_data = Arc::new(GameDataStore::new(&snapshot).unwrap());
        let clock = Arc::new(MockClock::new(Some(start_instant()), 0.0));
        (CodexService::new(pool.clone(), game_data, clock), pool)
    }

    /// The standard calibration seed: Rifle twice (the newer scan
    /// instant wins), plus Athletics, Dodge, and Agility anchors.
    async fn seed_calibrations(pool: &SqlitePool) {
        for (name, level, at) in [
            ("Rifle", 90.0, 100.0),
            ("Rifle", 100.0, 200.0),
            ("Athletics", 5.0, 150.0),
            ("Dodge", 30.0, 150.0),
            ("Agility", 32.04, 150.0),
        ] {
            sqlx::query(
                "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                 VALUES (?, ?, 'scan', ?)",
            )
            .bind(name)
            .bind(level)
            .bind(at)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    fn invalid(error: CodexError) -> String {
        match error {
            CodexError::Invalid(message) => message,
            CodexError::Db(error) => panic!("expected a validation error, got: {error}"),
        }
    }

    #[tokio::test]
    async fn species_listing_dedupes_skips_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;

        // Unranked: alphabetical (every rank ties at 0); the duplicate
        // Boar keeps its first base cost, the nameless/specieless rows
        // drop, and Ghost lists through its second (costed) entry.
        let initial = svc.get_all_species().await.unwrap();
        assert_eq!(
            initial,
            vec![
                json!({"name": "Boar", "baseCost": 37.5, "codexType": "Mob", "currentRank": 0,
                       "nextRank": 1, "nextCategory": "cat1", "nextCost": 37.5}),
                json!({"name": "Ghost", "baseCost": 7.0, "codexType": null, "currentRank": 0,
                       "nextRank": 1, "nextCategory": "cat1", "nextCost": 7.0}),
                json!({"name": "Looter Bird", "baseCost": 10.0, "codexType": "MobLooter",
                       "currentRank": 0, "nextRank": 1, "nextCategory": "cat1", "nextCost": 10.0}),
            ]
        );

        // Ranked: rank-descending, then name; the next-rank fields
        // derive from each species' own cost table.
        svc.calibrate("Looter Bird", 5).await.unwrap();
        svc.calibrate("Boar", 2).await.unwrap();
        let ranked = svc.get_all_species().await.unwrap();
        assert_eq!(
            ranked,
            vec![
                json!({"name": "Looter Bird", "baseCost": 10.0, "codexType": "MobLooter",
                       "currentRank": 5, "nextRank": 6, "nextCategory": "cat1", "nextCost": 80.0}),
                json!({"name": "Boar", "baseCost": 37.5, "codexType": "Mob", "currentRank": 2,
                       "nextRank": 3, "nextCategory": "cat2", "nextCost": 112.5}),
                json!({"name": "Ghost", "baseCost": 7.0, "codexType": null, "currentRank": 0,
                       "nextRank": 1, "nextCategory": "cat1", "nextCost": 7.0}),
            ]
        );

        // Rank 25 has no next rank.
        svc.calibrate("Boar", 25).await.unwrap();
        let maxed = svc.get_all_species().await.unwrap();
        assert_eq!(maxed[0]["name"], "Boar");
        assert_eq!(maxed[0]["nextRank"], Value::Null);
        assert_eq!(maxed[0]["nextCategory"], Value::Null);
        assert_eq!(maxed[0]["nextCost"], Value::Null);
    }

    #[tokio::test]
    async fn the_first_catalogue_match_decides_species_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;

        // Ghost's first catalogue entry has no base cost, so the
        // lookup paths miss it even though the listing carries it.
        assert_eq!(svc.get_species_ranks("Ghost").await.unwrap(), None);
        let error = svc.claim_rank("Ghost", 1, "Rifle").await.unwrap_err();
        assert_eq!(
            invalid(error),
            "Species 'Ghost' not found in game-data catalogue"
        );
        assert_eq!(svc.get_species_ranks("Nessie").await.unwrap(), None);
        assert_eq!(
            svc.get_skill_options("Nessie", 1, None, "profession")
                .await
                .unwrap(),
            Vec::<Value>::new()
        );
    }

    #[tokio::test]
    async fn rank_breakdowns_cross_reference_claims() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        seed_calibrations(&pool).await;
        svc.claim_rank("Boar", 1, "Rifle").await.unwrap();
        svc.claim_rank("Boar", 2, "Anatomy").await.unwrap();

        let ranks = svc.get_species_ranks("Boar").await.unwrap().unwrap();
        assert_eq!(ranks["speciesName"], "Boar");
        assert_eq!(ranks["baseCost"], json!(37.5));
        assert_eq!(ranks["codexType"], "Mob");
        assert_eq!(ranks["currentRank"], json!(2));
        let items = ranks["ranks"].as_array().unwrap();
        assert_eq!(items.len(), 25);

        assert_eq!(items[0]["rank"], json!(1));
        assert_eq!(items[0]["claimed"], json!(true));
        assert_eq!(items[0]["claimedSkill"], "Rifle");
        assert_eq!(items[0]["claimedPed"], json!(0.1875));
        assert_eq!(items[0]["isNext"], json!(false));
        assert_eq!(items[0]["cost"], json!(37.5));
        assert_eq!(items[0]["rewardPed"], json!(0.1875));

        assert_eq!(items[1]["claimedSkill"], "Anatomy");
        assert_eq!(items[1]["claimedPed"], json!(0.375));

        assert_eq!(items[2]["claimed"], json!(false));
        assert_eq!(items[2]["claimedSkill"], Value::Null);
        assert_eq!(items[2]["claimedPed"], Value::Null);
        assert_eq!(items[2]["isNext"], json!(true));
        assert!(items[3..].iter().all(|item| item["isNext"] == json!(false)));
    }

    #[tokio::test]
    async fn claims_validate_each_leg_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;

        let cases: [(&str, i64, &str, &str); 4] = [
            (
                "Nessie",
                1,
                "Rifle",
                "Species 'Nessie' not found in game-data catalogue",
            ),
            ("Boar", 5, "Rifle", "Expected rank 1, got 5"),
            (
                "Boar",
                3,
                "Rifle",
                "Skill 'Rifle' not valid for rank 3 (category cat2)",
            ),
            (
                "Looter Bird",
                1,
                "Zoology",
                "Skill 'Zoology' not valid for rank 1 (category cat1)",
            ),
        ];
        for (species, rank, skill, expected) in cases {
            if rank == 3 {
                svc.calibrate("Boar", 2).await.unwrap();
            }
            let error = svc.claim_rank(species, rank, skill).await.unwrap_err();
            assert_eq!(invalid(error), expected);
            if rank == 3 {
                svc.calibrate("Boar", 0).await.unwrap();
            }
        }

        // The max-rank leg sits behind the next-rank check, so it
        // fires only at the 25 -> 26 boundary.
        svc.calibrate("Looter Bird", 25).await.unwrap();
        let error = svc
            .claim_rank("Looter Bird", 26, "Evade")
            .await
            .unwrap_err();
        assert_eq!(invalid(error), "Maximum rank is 25");
    }

    #[tokio::test]
    async fn a_claim_records_progress_and_calibration() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        seed_calibrations(&pool).await;

        let result = svc.claim_rank("Boar", 1, "Rifle").await.unwrap();
        assert_eq!(
            result,
            json!({"speciesName": "Boar", "rank": 1, "skillName": "Rifle", "pedValue": 0.1875})
        );

        let now = naive_to_epoch(start_instant());
        let claim = sqlx::query(
            "SELECT species_name, rank, skill_name, ped_value, claimed_at, kind \
             FROM codex_claims",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(claim.get::<String, _>(0), "Boar");
        assert_eq!(claim.get::<i64, _>(1), 1);
        assert_eq!(claim.get::<String, _>(2), "Rifle");
        assert_eq!(claim.get::<f64, _>(3), 0.1875);
        assert_eq!(claim.get::<f64, _>(4), now);
        assert_eq!(claim.get::<String, _>(5), "rank");

        let progress =
            sqlx::query("SELECT current_rank FROM codex_progress WHERE species_name = 'Boar'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(progress.get::<i64, _>(0), 1);

        // The reward priced onto the curve from the NEWEST calibration
        // (level 100, not the older 90): 100 + levels bought by 0.1875
        // PED, the original's computed 217.745.
        let calibration =
            sqlx::query("SELECT level, scanned_at FROM skill_calibrations WHERE source = 'codex'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(calibration.get::<f64, _>(0), 217.745);
        assert_eq!(calibration.get::<f64, _>(1), now);

        // The next claim builds on the advanced rank.
        let result = svc.claim_rank("Boar", 2, "Anatomy").await.unwrap();
        assert_eq!(result["pedValue"], json!(0.375));
    }

    #[tokio::test]
    async fn an_uncalibrated_skill_claim_skips_the_calibration_write() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        seed_calibrations(&pool).await;

        svc.claim_rank("Boar", 1, "Rifle").await.unwrap();
        svc.claim_rank("Boar", 2, "Anatomy").await.unwrap();

        // Five seeds plus Rifle's codex row; Anatomy (no calibration
        // history) recorded its claim but wrote no calibration: the
        // reward never reaches the skill curve (see the module doc).
        let count = sqlx::query("SELECT COUNT(*) FROM skill_calibrations")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get::<i64, _>(0);
        assert_eq!(count, 6);
        let claims = sqlx::query("SELECT COUNT(*) FROM codex_claims")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get::<i64, _>(0);
        assert_eq!(claims, 2);
    }

    #[tokio::test]
    async fn cat4_claims_price_through_the_cat4_divisor() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;

        svc.calibrate("Looter Bird", 4).await.unwrap();
        let result = svc.claim_rank("Looter Bird", 5, "Zoology").await.unwrap();
        assert_eq!(
            result,
            json!({"speciesName": "Looter Bird", "rank": 5, "skillName": "Zoology",
                   "pedValue": 0.06})
        );
    }

    #[tokio::test]
    async fn calibrate_bounds_and_upserts() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;

        for rank in [-1, 26] {
            let error = svc.calibrate("Boar", rank).await.unwrap_err();
            assert_eq!(invalid(error), "Rank must be 0-25");
        }

        let result = svc.calibrate("Boar", 4).await.unwrap();
        assert_eq!(result, json!({"speciesName": "Boar", "rank": 4}));
        svc.calibrate("Boar", 7).await.unwrap();
        let rows =
            sqlx::query("SELECT current_rank FROM codex_progress WHERE species_name = 'Boar'")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 1, "the upsert overwrites in place");
        assert_eq!(rows[0].get::<i64, _>(0), 7);

        // Calibration is catalogue-blind and side-effect-free.
        svc.calibrate("Nessie", 0).await.unwrap();
    }

    #[tokio::test]
    async fn meta_claims_validate_record_and_report() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        seed_calibrations(&pool).await;

        let error = svc.meta_claim("Luck").await.unwrap_err();
        assert_eq!(
            invalid(error),
            "'Luck' is not an attribute. \
             Valid: ['Agility', 'Health', 'Intelligence', 'Psyche', 'Stamina', 'Strength']"
        );

        let result = svc.meta_claim("Agility").await.unwrap();
        assert_eq!(result, json!({"attributeName": "Agility", "pedValue": 1.0}));
        let row = sqlx::query(
            "SELECT species_name, rank, skill_name, ped_value, kind, attribute_name \
             FROM codex_claims WHERE kind = 'meta'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>(0), "__meta__");
        assert_eq!(row.get::<i64, _>(1), 0);
        assert_eq!(row.get::<String, _>(2), "Agility");
        assert_eq!(row.get::<f64, _>(3), 1.0);
        assert_eq!(row.get::<String, _>(5), "Agility");

        // The six attributes in sorted order, levels from the latest
        // calibration rounded to one decimal (32.04 -> 32.0).
        let attributes = svc.get_meta_attributes().await.unwrap();
        assert_eq!(
            attributes,
            vec![
                json!({"name": "Agility", "currentLevel": 32.0}),
                json!({"name": "Health", "currentLevel": null}),
                json!({"name": "Intelligence", "currentLevel": null}),
                json!({"name": "Psyche", "currentLevel": null}),
                json!({"name": "Stamina", "currentLevel": null}),
                json!({"name": "Strength", "currentLevel": null}),
            ]
        );
    }

    #[tokio::test]
    async fn the_final_rank_claims_at_the_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;

        // Rank 25 itself is claimable; the max-rank guard rejects only
        // beyond the table. Hand-computed reward: multiplier 100 x
        // base 37.5 = 3750 kill cost, cat3 divisor 640 -> 5.859375 ->
        // 5.8594 at four places.
        svc.calibrate("Boar", 24).await.unwrap();
        let result = svc.claim_rank("Boar", 25, "Evade").await.unwrap();
        assert_eq!(
            result,
            json!({"speciesName": "Boar", "rank": 25, "skillName": "Evade", "pedValue": 5.8594})
        );
    }

    #[test]
    fn errors_display_their_messages() {
        assert_eq!(
            CodexError::Invalid("Maximum rank is 25".to_string()).to_string(),
            "Maximum rank is 25"
        );
        assert_eq!(
            CodexError::Db(sqlx::Error::RowNotFound).to_string(),
            sqlx::Error::RowNotFound.to_string()
        );
    }

    #[tokio::test]
    async fn profession_options_rank_by_contribution() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        seed_calibrations(&pool).await;
        // Rifle advances to 217.745 through the claim, matching the
        // original's fixture sequence.
        svc.claim_rank("Boar", 1, "Rifle").await.unwrap();

        let options = svc
            .get_skill_options("Boar", 1, Some("Sniper"), "profession")
            .await
            .unwrap();
        assert_eq!(options.len(), 15);

        // Weighted skills lead, ranked by contribution computed from
        // the UNROUNDED levels (0.28749, not 0.2875 of the displayed
        // 143.75); the zero-contribution tail keeps category order
        // (the stable sort), with recommendRank withheld.
        assert_eq!(
            options[0],
            json!({"skillName": "Rifle", "category": "cat1", "rewardPed": 0.1875,
                   "currentLevel": 217.7, "levelsGained": 93.75, "professionWeight": 50,
                   "profContribution": 0.46875, "hpIncrease": null, "hpGain": 0.0,
                   "recommendRank": 1})
        );
        assert_eq!(
            options[1],
            json!({"skillName": "Aim", "category": "cat1", "rewardPed": 0.1875,
                   "currentLevel": null, "levelsGained": 143.75, "professionWeight": 20,
                   "profContribution": 0.28749, "hpIncrease": null, "hpGain": 0.0,
                   "recommendRank": 2})
        );
        assert_eq!(
            options[3],
            json!({"skillName": "Athletics", "category": "cat1", "rewardPed": 0.1875,
                   "currentLevel": 5.0, "levelsGained": 145.75, "professionWeight": 0,
                   "profContribution": 0.0, "hpIncrease": 20.0, "hpGain": 7.28725,
                   "recommendRank": null})
        );
        let names: Vec<&str> = options
            .iter()
            .map(|option| option["skillName"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "Rifle",
                "Aim",
                "Anatomy",
                "Athletics",
                "BLP Weaponry Technology",
                "Combat Reflexes",
                "Dexterity",
                "Handgun",
                "Heavy Melee Weapons",
                "Laser Weaponry Technology",
                "Light Melee Weapons",
                "Longblades",
                "Power Fist",
                "Shortblades",
                "Weapons Handling",
            ]
        );
        assert!(options[2..]
            .iter()
            .all(|option| option["recommendRank"] == Value::Null));
    }

    #[tokio::test]
    async fn hp_options_sort_by_gain_then_level_then_name() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        seed_calibrations(&pool).await;

        // A MobLooter rank 5 offers cat3 plus the cat4 bonus skills.
        let options = svc
            .get_skill_options("Looter Bird", 5, None, "hp")
            .await
            .unwrap();
        assert_eq!(options.len(), 19);

        assert_eq!(
            options[0],
            json!({"skillName": "Zoology", "category": "cat4", "rewardPed": 0.06,
                   "currentLevel": null, "levelsGained": 44.99, "professionWeight": 0,
                   "profContribution": 0.0, "hpIncrease": 5.5, "hpGain": 8.180909,
                   "recommendRank": 1})
        );
        assert_eq!(
            options[1],
            json!({"skillName": "Dodge", "category": "cat3", "rewardPed": 0.0938,
                   "currentLevel": 30.0, "levelsGained": 78.38, "professionWeight": 0,
                   "profContribution": 0.0, "hpIncrease": 12.0, "hpGain": 6.53125,
                   "recommendRank": 2})
        );

        // The zero-gain tail interleaves cat3 and cat4 alphabetically
        // (every key ties, so the name decides).
        let names: Vec<&str> = options
            .iter()
            .map(|option| option["skillName"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "Zoology",
                "Dodge",
                "Alertness",
                "Analysis",
                "Animal Lore",
                "Biology",
                "Bioregenesis",
                "Botany",
                "Bravado",
                "Computer",
                "Concentration",
                "Evade",
                "Explosive Projectile Weaponry Technology",
                "First Aid",
                "Heavy Weapons",
                "Support Weapon Systems",
                "Telepathy",
                "Translocation",
                "Vehicle Repairing",
            ]
        );
        assert!(options[2..]
            .iter()
            .all(|option| option["recommendRank"] == Value::Null));
    }

    #[tokio::test]
    async fn concurrent_claims_record_only_one_rank() {
        // Two concurrent claims for the same next rank must not both
        // succeed: the conditional progress upsert advances exactly one
        // and the loser aborts, so no rank is double-credited. The race
        // runs over many fresh databases; tokio::join! interleaves the
        // two claims' validation reads before either writes (the precise
        // check-then-act window), so the pre-fix unconditional upsert
        // double-records and this invariant fails.
        for _ in 0..32 {
            let dir = tempfile::tempdir().unwrap();
            let (svc, pool) = service(dir.path()).await;
            seed_calibrations(&pool).await;

            let (a, b) = tokio::join!(
                svc.claim_rank("Boar", 1, "Rifle"),
                svc.claim_rank("Boar", 1, "Rifle"),
            );
            assert!(
                a.is_ok() ^ b.is_ok(),
                "exactly one claim must win: a={a:?} b={b:?}"
            );

            let claims: i64 = sqlx::query(
                "SELECT COUNT(*) FROM codex_claims WHERE species_name = 'Boar' AND rank = 1",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
            assert_eq!(claims, 1, "exactly one claim row may be recorded");

            let progress: i64 =
                sqlx::query("SELECT current_rank FROM codex_progress WHERE species_name = 'Boar'")
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                    .get(0);
            assert_eq!(progress, 1, "progress advances exactly once");
        }
    }

    #[tokio::test]
    async fn unclaim_reverts_progress_claim_and_calibration() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        seed_calibrations(&pool).await;

        // A claim advances to rank 1, records the claim, and writes a
        // codex calibration on top of Rifle's newest level (100).
        svc.claim_rank("Boar", 1, "Rifle").await.unwrap();
        assert_eq!(svc.current_rank("Boar").await.unwrap(), 1);
        assert_eq!(svc.skill_level("Rifle").await.unwrap(), Some(217.745));

        let reverted = svc.unclaim_rank("Boar").await.unwrap();
        assert_eq!(
            reverted,
            json!({"speciesName": "Boar", "rank": 1, "skillName": "Rifle", "pedValue": 0.1875})
        );

        // Rank steps back, the claim row is gone, and the codex
        // calibration is removed so Rifle reverts to its scanned 100;
        // the five seed rows are untouched.
        assert_eq!(svc.current_rank("Boar").await.unwrap(), 0);
        let claims: i64 = sqlx::query("SELECT COUNT(*) FROM codex_claims")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(claims, 0);
        let codex_rows: i64 =
            sqlx::query("SELECT COUNT(*) FROM skill_calibrations WHERE source = 'codex'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(codex_rows, 0);
        let total: i64 = sqlx::query("SELECT COUNT(*) FROM skill_calibrations")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(total, 5, "the scan-seeded calibrations are untouched");
        assert_eq!(svc.skill_level("Rifle").await.unwrap(), Some(100.0));
    }

    #[tokio::test]
    async fn unclaim_of_an_uncalibrated_skill_claim_removes_only_the_claim() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        seed_calibrations(&pool).await;

        // Anatomy has no calibration history, so its claim wrote none
        // (the documented silent skip); unclaiming it must not touch
        // Rifle's codex calibration from the earlier rank.
        svc.claim_rank("Boar", 1, "Rifle").await.unwrap();
        svc.claim_rank("Boar", 2, "Anatomy").await.unwrap();

        svc.unclaim_rank("Boar").await.unwrap();

        assert_eq!(svc.current_rank("Boar").await.unwrap(), 1);
        let claims: i64 = sqlx::query("SELECT COUNT(*) FROM codex_claims")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(claims, 1, "only the Anatomy claim is removed");
        let codex_rows: i64 =
            sqlx::query("SELECT COUNT(*) FROM skill_calibrations WHERE source = 'codex'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(codex_rows, 1, "Rifle's codex calibration survives");
    }

    #[tokio::test]
    async fn unclaim_requires_a_claimed_latest_rank() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;

        // Nothing claimed at all.
        let error = svc.unclaim_rank("Boar").await.unwrap_err();
        assert_eq!(invalid(error), "No claimed rank to unclaim for 'Boar'");

        // A rank reached by manual calibration carries no claim to
        // revert; unclaim refuses rather than silently stepping back.
        svc.calibrate("Boar", 3).await.unwrap();
        let error = svc.unclaim_rank("Boar").await.unwrap_err();
        assert_eq!(invalid(error), "Rank 3 for 'Boar' was not claimed");
        assert_eq!(svc.current_rank("Boar").await.unwrap(), 3);
    }

    #[tokio::test]
    async fn concurrent_unclaims_revert_only_once() {
        // Two concurrent unclaims of the same claimed rank must not both
        // succeed: the conditional rank step-back fires for exactly one,
        // the loser aborts before deleting, so the claim is reverted
        // once and never double-stepped.
        for _ in 0..32 {
            let dir = tempfile::tempdir().unwrap();
            let (svc, pool) = service(dir.path()).await;
            seed_calibrations(&pool).await;
            svc.claim_rank("Boar", 1, "Rifle").await.unwrap();

            let (a, b) = tokio::join!(svc.unclaim_rank("Boar"), svc.unclaim_rank("Boar"));
            assert!(
                a.is_ok() ^ b.is_ok(),
                "exactly one unclaim must win: a={a:?} b={b:?}"
            );

            let claims: i64 = sqlx::query(
                "SELECT COUNT(*) FROM codex_claims WHERE species_name = 'Boar' AND rank = 1",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
            assert_eq!(claims, 0, "the single claim is reverted exactly once");

            let progress: i64 =
                sqlx::query("SELECT current_rank FROM codex_progress WHERE species_name = 'Boar'")
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                    .get(0);
            assert_eq!(progress, 0, "rank steps back exactly once");
        }
    }
}
