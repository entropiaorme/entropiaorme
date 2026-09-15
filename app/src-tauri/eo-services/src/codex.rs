//! Codex service: species listing, rank breakdowns, claim recording, manual rank
//! calibration, per-rank skill recommendations, and the meta
//! (attribute) codex.
//!
//! Species data comes from the bundled game-data catalogue; player
//! progress and claims live in the application database; claim and
//! calibration timestamps stamp from the injected clock.
//!
//! One inherited behaviour is kept deliberately rather than repaired,
//! pinned by the frozen goldens (ADR-0017):
//!
//! - **Silent calibration skip.** A claimed reward only lands in
//!   `skill_calibrations` when the skill already has a calibration
//!   history; a first-ever claim for an uncalibrated skill records
//!   the claim and updates progress but writes no calibration row,
//!   so the reward's levels never reach the skill curve.
//!
//! The inherited check-then-act claim validation (it read
//! `current_rank` and validated before taking its database lock, so
//! two racing claims for the same rank could both observe it and both
//! record a reward) is NOT reproduced: `claim_rank` advances progress
//! with a conditional upsert gated on the prior rank, so of two racing
//! claims exactly one advances and the loser aborts. In serial use the
//! guard always holds, so single-threaded behaviour is unchanged (the
//! banked equivalence evidence confirmed it) while the race is closed.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rusqlite::OptionalExtension as _;
use serde::Serialize;
use serde_json::Value;

use crate::clock::Clock;
use crate::codex_categories::{
    build_rank_breakdown, get_category_for_rank, get_rank_cost, get_reward_ped, is_cat4_rank,
    mastery_reward_ped, skills_for_category, RankBreakdown, CAT4_SKILLS, MASTERY_CATEGORIES,
};
use crate::db::Db;
use crate::game_data_store::GameDataStore;
use crate::time::naive_to_epoch;
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

/// The service's error surface: `Invalid` carries validation-refusal
/// messages verbatim, as pinned by the frozen goldens; the facade maps
/// it to `ApiError::BadRequest`. `Rollup` is a database failure.
#[derive(Debug, thiserror::Error)]
pub enum CodexError {
    #[error("{0}")]
    Invalid(String),
    /// A daily-rollup refresh failure inside a claim write.
    #[error(transparent)]
    Rollup(#[from] crate::db::DbError),
}

/// A species' codex parameters from the game-data catalogue.
struct Species {
    base_cost: f64,
    codex_type: Option<String>,
}

/// One species in the codex listing. Serialises to the wire shape the
/// goldens pin (camelCase, declaration order).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeciesEntry {
    pub name: String,
    pub base_cost: f64,
    pub codex_type: Option<String>,
    pub current_rank: i64,
    pub next_rank: Option<i64>,
    pub next_category: Option<&'static str>,
    pub next_cost: Option<f64>,
    pub mastery_level: i64,
}

/// One rank of a species' breakdown with the player's claim overlay:
/// the breakdown's own fields first, then the overlay (the wire order).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankEntry {
    #[serde(flatten)]
    pub breakdown: RankBreakdown,
    pub claimed: bool,
    pub claimed_skill: Option<String>,
    pub claimed_ped: Option<f64>,
    pub is_next: bool,
}

/// A species' full 25-rank breakdown, cross-referenced with the
/// player's claims and current rank.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeciesRanks {
    pub species_name: String,
    pub base_cost: f64,
    pub codex_type: Option<String>,
    pub current_rank: i64,
    pub mastery_level: i64,
    pub mastery_claims: Vec<MasteryClaimEntry>,
    pub ranks: Vec<RankEntry>,
}

/// One requested profession's share of a skill option's contribution
/// (professions with no weight on the skill are omitted).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfessionContribution {
    pub profession: String,
    pub prof_contribution: f64,
}

/// One skill option in a rank recommendation. Numeric fields hold the
/// rendered (rounded) figures, so ordering and output read the same
/// values. `profession_weight` / `prof_contribution` are summed over
/// the requested professions; `profession_contributions` carries the
/// per-profession split (one entry per weighted requested profession).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillOption {
    pub skill_name: &'static str,
    pub category: &'static str,
    pub reward_ped: f64,
    pub current_level: Option<f64>,
    pub levels_gained: f64,
    pub profession_weight: i64,
    pub prof_contribution: f64,
    pub profession_contributions: Vec<ProfessionContribution>,
    pub hp_increase: Option<f64>,
    pub hp_gain: f64,
    pub recommend_rank: Option<i64>,
}

/// One recorded mastery claim in a species' breakdown, in claim order
/// (`mastery_level` is the 1-based per-species sequence number).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MasteryClaimEntry {
    pub mastery_level: i64,
    pub skill_name: String,
    pub ped_value: f64,
}

/// One meta attribute with its current calibrated level.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaAttribute {
    pub name: &'static str,
    pub current_level: Option<f64>,
}

/// The record a rank claim (or its reversal) reports.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimRecord {
    pub species_name: String,
    pub rank: i64,
    pub skill_name: String,
    pub ped_value: f64,
}

/// The record a manual rank calibration reports.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrateRecord {
    pub species_name: String,
    pub rank: i64,
}

/// The record a meta claim reports.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaClaimRecord {
    pub attribute_name: String,
    pub ped_value: f64,
}

/// The record a mastery claim (or its reversal) reports.
/// `mastery_level` is the per-species claim sequence number (the Nth
/// mastery claim for that species).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MasteryClaimRecord {
    pub species_name: String,
    pub mastery_level: i64,
    pub skill_name: String,
    pub ped_value: f64,
}

/// Codex operations: species listing, rank breakdowns, claim recording.
pub struct CodexService {
    db: Db,
    game_data: Arc<GameDataStore>,
    clock: Arc<dyn Clock>,
}

/// The result of the `unclaim_rank` writer transaction: the validation
/// branches the original raised as `ValueError`s (surfaced by the caller as
/// [`CodexError::Invalid`]), or the completed revert carrying the claim's
/// fields for the response. The transaction runs on the synchronous core, so
/// its closure returns only [`crate::db::DbError`]; the domain outcome travels
/// out as this value.
enum UnclaimOutcome {
    NoRank,
    NotClaimed {
        rank: i64,
    },
    AlreadyUnclaimed {
        rank: i64,
    },
    MasteryBlocks {
        mastery_level: i64,
    },
    Done {
        rank: i64,
        skill_name: String,
        ped_value: f64,
    },
}

/// The result of the `mastery_claim` writer transaction: the rank-25
/// gate re-checked inside the transaction (the authoritative read; the
/// pre-check outside it is advisory), or the completed claim carrying
/// its sequence number.
enum MasteryClaimOutcome {
    NotAt25 { current_rank: i64 },
    Done { mastery_level: i64 },
}

/// The result of the `mastery_unclaim` writer transaction.
enum MasteryUnclaimOutcome {
    NothingClaimed,
    Done {
        mastery_level: i64,
        skill_name: String,
        ped_value: f64,
    },
}

impl CodexService {
    pub fn new(db: Db, game_data: Arc<GameDataStore>, clock: Arc<dyn Clock>) -> Self {
        Self {
            db,
            game_data,
            clock,
        }
    }

    /// The database handle, for tests that drive the synchronous core
    /// (the rollup heal) directly.
    #[cfg(test)]
    fn db(&self) -> &Db {
        &self.db
    }

    /// All mob species with a codex base cost, cross-referenced with
    /// player rank, sorted rank-descending then name-ascending.
    pub async fn get_all_species(&self) -> Result<Vec<SpeciesEntry>, CodexError> {
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

        let (rank_map, mastery_map): (HashMap<String, i64>, HashMap<String, i64>) = self
            .db
            .with_reader(|conn| {
                let mut stmt =
                    conn.prepare("SELECT species_name, current_rank FROM codex_progress")?;
                let mut rows = stmt.query([])?;
                let mut ranks: HashMap<String, i64> = HashMap::new();
                while let Some(row) = rows.next()? {
                    ranks.insert(row.get::<_, String>(0)?, row.get::<_, i64>(1)?);
                }
                let mut stmt = conn.prepare(
                    "SELECT species_name, COUNT(*) FROM codex_claims \
                     WHERE kind = 'mastery' GROUP BY species_name",
                )?;
                let mut rows = stmt.query([])?;
                let mut masteries: HashMap<String, i64> = HashMap::new();
                while let Some(row) = rows.next()? {
                    masteries.insert(row.get::<_, String>(0)?, row.get::<_, i64>(1)?);
                }
                Ok((ranks, masteries))
            })
            .await?;

        let mut result: Vec<SpeciesEntry> = Vec::new();
        for (name, base_cost, codex_type) in listed {
            let rank = rank_map.get(&name).copied().unwrap_or(0);
            let mastery_level = mastery_map.get(&name).copied().unwrap_or(0);
            let next_rank = if rank < 25 { Some(rank + 1) } else { None };
            // The derived fields gate on the next rank's truthiness (an
            // inherited rule), so a (hand-edited) rank of -1 yields a
            // nextRank of 0 with no category or cost.
            let derivable = next_rank.filter(|&next| next != 0);
            result.push(SpeciesEntry {
                name,
                base_cost,
                codex_type,
                current_rank: rank,
                next_rank,
                next_category: derivable.map(get_category_for_rank),
                next_cost: derivable.map(|next| round_half_even(get_rank_cost(next, base_cost), 2)),
                mastery_level,
            });
        }
        result.sort_by(|a, b| {
            b.current_rank
                .cmp(&a.current_rank)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(result)
    }

    /// The 25-rank breakdown for a species, cross-referenced with
    /// claims; `None` when the species is not in the catalogue.
    pub async fn get_species_ranks(
        &self,
        species_name: &str,
    ) -> Result<Option<SpeciesRanks>, CodexError> {
        let Some(species) = self.find_species(species_name) else {
            return Ok(None);
        };
        let breakdown = build_rank_breakdown(species.base_cost, species.codex_type.as_deref());

        let species_owned = species_name.to_string();
        // Built in query order so a duplicate rank's later row wins,
        // as the original's dict comprehension does. Rank claims only:
        // mastery claims share the table and species name but reuse the
        // rank column as their own sequence number, so they list
        // separately, in claim order.
        let (claims_map, mastery_claims): (HashMap<i64, (String, f64)>, Vec<MasteryClaimEntry>) =
            self.db
                .with_reader(move |conn| {
                    let mut stmt = conn.prepare(
                        "SELECT rank, skill_name, ped_value, claimed_at FROM codex_claims \
                         WHERE species_name = ? AND kind = 'rank' ORDER BY rank",
                    )?;
                    let mut rows = stmt.query(rusqlite::params![&species_owned])?;
                    let mut map: HashMap<i64, (String, f64)> = HashMap::new();
                    while let Some(row) = rows.next()? {
                        map.insert(
                            row.get::<_, i64>(0)?,
                            (row.get::<_, String>(1)?, row.get::<_, f64>(2)?),
                        );
                    }
                    let mut stmt = conn.prepare(
                        "SELECT rank, skill_name, ped_value FROM codex_claims \
                         WHERE species_name = ? AND kind = 'mastery' ORDER BY rank",
                    )?;
                    let mut rows = stmt.query(rusqlite::params![&species_owned])?;
                    let mut mastery: Vec<MasteryClaimEntry> = Vec::new();
                    while let Some(row) = rows.next()? {
                        mastery.push(MasteryClaimEntry {
                            mastery_level: row.get::<_, i64>(0)?,
                            skill_name: row.get::<_, String>(1)?,
                            ped_value: row.get::<_, f64>(2)?,
                        });
                    }
                    Ok((map, mastery))
                })
                .await?;

        let current_rank = self.current_rank(species_name).await?;

        let ranks: Vec<RankEntry> = breakdown
            .into_iter()
            .map(|item| {
                let claim = claims_map.get(&item.rank);
                let is_next = item.rank == current_rank + 1;
                RankEntry {
                    claimed: claim.is_some(),
                    claimed_skill: claim.map(|(skill, _)| skill.clone()),
                    claimed_ped: claim.map(|&(_, ped)| ped),
                    is_next,
                    breakdown: item,
                }
            })
            .collect();

        Ok(Some(SpeciesRanks {
            species_name: species_name.to_string(),
            base_cost: species.base_cost,
            codex_type: species.codex_type,
            current_rank,
            mastery_level: mastery_claims.len() as i64,
            mastery_claims,
            ranks,
        }))
    }

    /// Claim a codex rank reward: validates, records the claim,
    /// advances progress, and updates the skill calibration.
    pub async fn claim_rank(
        &self,
        species_name: &str,
        rank: i64,
        skill_name: &str,
    ) -> Result<ClaimRecord, CodexError> {
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
        let species_owned = species_name.to_string();
        let skill_owned = skill_name.to_string();
        let advanced = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let advanced = tx.execute(
                    "INSERT INTO codex_progress (species_name, current_rank, updated_at) \
                     VALUES (?, ?, ?) \
                     ON CONFLICT(species_name) DO UPDATE SET current_rank = ?, updated_at = ? \
                     WHERE codex_progress.current_rank = ? - 1",
                    rusqlite::params![species_owned, rank, now, rank, now, rank],
                )?;
                if advanced == 0 {
                    // Another claim advanced this species' rank between our
                    // validation read and this write; abort as the race loser
                    // (the transaction rolls back on drop, so nothing lands).
                    return Ok(false);
                }

                tx.execute(
                    "INSERT INTO codex_claims \
                     (species_name, rank, skill_name, ped_value, claimed_at, kind) \
                     VALUES (?, ?, ?, ?, ?, 'rank')",
                    rusqlite::params![species_owned, rank, skill_owned, ped_value, now],
                )?;
                crate::daily_rollup::refresh_days(&tx, [crate::daily_rollup::epoch_day(now)])?;

                write_codex_calibration(&tx, &skill_owned, ped_value, now)?;
                tx.commit()?;
                Ok(true)
            })
            .await?;
        if !advanced {
            return Err(CodexError::Invalid(format!(
                "Rank {rank} for '{species_name}' was already claimed"
            )));
        }

        Ok(ClaimRecord {
            species_name: species_name.to_string(),
            rank,
            skill_name: skill_name.to_string(),
            ped_value,
        })
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
    /// A forward feature with no Python-era original; it was mirrored in
    /// the retired oracle so the OpenAPI contract carries the route, but the
    /// cross-language differential never drove it.
    pub async fn unclaim_rank(&self, species_name: &str) -> Result<ClaimRecord, CodexError> {
        let now = naive_to_epoch(self.clock.now());
        let species_owned = species_name.to_string();
        let outcome = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;

                let current_rank = tx
                    .query_row(
                        "SELECT current_rank FROM codex_progress WHERE species_name = ?",
                        rusqlite::params![species_owned],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                    .unwrap_or(0);
                if current_rank < 1 {
                    return Ok(UnclaimOutcome::NoRank);
                }
                let rank = current_rank;

                let claim: Option<(String, f64, f64)> = tx
                    .query_row(
                        "SELECT skill_name, ped_value, claimed_at FROM codex_claims \
                         WHERE species_name = ? AND rank = ? AND kind = 'rank'",
                        rusqlite::params![species_owned, rank],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, f64>(1)?,
                                row.get::<_, f64>(2)?,
                            ))
                        },
                    )
                    .optional()?;
                let Some((skill_name, ped_value, claimed_at)) = claim else {
                    return Ok(UnclaimOutcome::NotClaimed { rank });
                };

                // Mastery history sits on top of rank 25: its claims (and
                // any calibrations they priced from this claim's level)
                // depend on the species being complete, so the rank-25
                // claim only reverts once the mastery claims are undone.
                let mastery_level: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM codex_claims \
                     WHERE species_name = ? AND kind = 'mastery'",
                    rusqlite::params![species_owned],
                    |row| row.get(0),
                )?;
                if mastery_level > 0 {
                    return Ok(UnclaimOutcome::MasteryBlocks { mastery_level });
                }

                // Step the rank back, gated on it still being `rank`, so of two
                // racing unclaims exactly one steps it and the loser aborts
                // before deleting anything (the mirror of claim_rank's guard).
                let stepped = tx.execute(
                    "UPDATE codex_progress SET current_rank = ?, updated_at = ? \
                     WHERE species_name = ? AND current_rank = ?",
                    rusqlite::params![rank - 1, now, species_owned, rank],
                )?;
                if stepped == 0 {
                    return Ok(UnclaimOutcome::AlreadyUnclaimed { rank });
                }

                // Remove the codex-sourced calibration this claim wrote, matched
                // on the instant the claim and calibration inserts share; the
                // id-subquery removes at most one row, and an uncalibrated-skill
                // claim (which wrote none) removes nothing here.
                // id-order: tiebreak (rows already narrowed to one scanned_at).
                tx.execute(
                    "DELETE FROM skill_calibrations WHERE id = ( \
                        SELECT id FROM skill_calibrations \
                        WHERE skill_name = ? AND source = 'codex' AND scanned_at = ? \
                        ORDER BY id DESC LIMIT 1)",
                    rusqlite::params![skill_name, claimed_at],
                )?;

                tx.execute(
                    "DELETE FROM codex_claims WHERE species_name = ? AND rank = ? AND kind = 'rank'",
                    rusqlite::params![species_owned, rank],
                )?;
                // The unclaimed reward may sit days back; reland its day's
                // rollup inside the same transaction.
                crate::daily_rollup::refresh_days(
                    &tx,
                    [crate::daily_rollup::epoch_day(claimed_at)],
                )?;

                tx.commit()?;
                Ok(UnclaimOutcome::Done {
                    rank,
                    skill_name,
                    ped_value,
                })
            })
            .await?;

        match outcome {
            UnclaimOutcome::NoRank => Err(CodexError::Invalid(format!(
                "No claimed rank to unclaim for '{species_name}'"
            ))),
            UnclaimOutcome::NotClaimed { rank } => Err(CodexError::Invalid(format!(
                "Rank {rank} for '{species_name}' was not claimed"
            ))),
            UnclaimOutcome::AlreadyUnclaimed { rank } => Err(CodexError::Invalid(format!(
                "Rank {rank} for '{species_name}' was already unclaimed"
            ))),
            UnclaimOutcome::MasteryBlocks { mastery_level } => Err(CodexError::Invalid(format!(
                "Undo the {mastery_level} mastery claim(s) for '{species_name}' before unclaiming rank 25"
            ))),
            UnclaimOutcome::Done {
                rank,
                skill_name,
                ped_value,
            } => Ok(ClaimRecord {
                species_name: species_name.to_string(),
                rank,
                skill_name,
                ped_value,
            }),
        }
    }

    /// Set the codex rank directly, no side effects (manual
    /// calibration).
    pub async fn calibrate(
        &self,
        species_name: &str,
        rank: i64,
    ) -> Result<CalibrateRecord, CodexError> {
        if !(0..=25).contains(&rank) {
            return Err(CodexError::Invalid("Rank must be 0-25".to_string()));
        }
        let now = naive_to_epoch(self.clock.now());
        let species_owned = species_name.to_string();
        self.db
            .with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO codex_progress (species_name, current_rank, updated_at) \
                     VALUES (?, ?, ?) \
                     ON CONFLICT(species_name) DO UPDATE SET current_rank = ?, updated_at = ?",
                    rusqlite::params![species_owned, rank, now, rank, now],
                )?;
                Ok(())
            })
            .await?;
        Ok(CalibrateRecord {
            species_name: species_name.to_string(),
            rank,
        })
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
        professions: &[String],
        target: &str,
    ) -> Result<Vec<SkillOption>, CodexError> {
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

        self.rank_options(skill_entries, professions, target).await
    }

    /// Skill choices for the mastery reward, ranked exactly as the
    /// per-rank recommendation is. Species-independent: the eligible
    /// skills and their fixed rewards are the same for every species.
    pub async fn get_mastery_skill_options(
        &self,
        professions: &[String],
        target: &str,
    ) -> Result<Vec<SkillOption>, CodexError> {
        let mut skill_entries: Vec<(&'static str, &'static str, f64)> = Vec::new();
        for category in MASTERY_CATEGORIES {
            for &skill_name in skills_for_category(category).expect("known category") {
                let ped = mastery_reward_ped(skill_name).expect("mastery-eligible category");
                skill_entries.push((skill_name, category, ped));
            }
        }
        self.rank_options(skill_entries, professions, target).await
    }

    /// Enrich `(skill, category, reward)` entries with the current
    /// calibrated level, the levels the reward buys on the curve, and
    /// the profession / HP contribution, then sort and assign the
    /// 1-based recommendation rank (the shared tail of the per-rank and
    /// mastery recommendations).
    async fn rank_options(
        &self,
        skill_entries: Vec<(&'static str, &'static str, f64)>,
        professions: &[String],
        target: &str,
    ) -> Result<Vec<SkillOption>, CodexError> {
        // Profession weights: per requested (non-empty) profession, the
        // first matching catalogue entry's skill list, weight defaults
        // applied as the original's `or 0`. Several professions (a
        // family targeted together) sum their weights per skill, which
        // ranks by total progress across the family: contribution is
        // linear in weight, so a summed map equals summed per-profession
        // contributions. One profession reproduces the original ranking
        // exactly.
        let mut weight_map: HashMap<&str, i64> = HashMap::new();
        let mut requested: Vec<&str> = Vec::new();
        for name in professions.iter().filter(|name| !name.is_empty()) {
            if !requested.contains(&name.as_str()) {
                requested.push(name.as_str());
            }
        }
        let mut per_profession: Vec<(String, HashMap<&str, i64>)> = Vec::new();
        for profession in requested {
            for entry in self.game_data.get_entities("professions") {
                if entry.get("name").and_then(Value::as_str) != Some(profession) {
                    continue;
                }
                // Within one profession a duplicate skill entry's later
                // weight wins (the original's dict insertion), so the
                // profession resolves its own map before summing in.
                let mut profession_map: HashMap<&str, i64> = HashMap::new();
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
                        profession_map.insert(name, weight);
                    }
                }
                for (&name, &weight) in &profession_map {
                    *weight_map.entry(name).or_insert(0) += weight;
                }
                per_profession.push((profession.to_string(), profession_map));
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

        let mut skills: Vec<SkillOption> = Vec::new();
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
            let profession_contributions: Vec<ProfessionContribution> = per_profession
                .iter()
                .filter_map(|(profession, map)| {
                    let weight = map.get(skill_name).copied().unwrap_or(0);
                    (weight > 0).then(|| ProfessionContribution {
                        profession: profession.clone(),
                        prof_contribution: round_half_even(
                            levels_gained * weight as f64 / 10000.0,
                            6,
                        ),
                    })
                })
                .collect();

            skills.push(SkillOption {
                skill_name,
                category: cat,
                reward_ped: ped,
                current_level: current_level.map(|level| round_half_even(level, 1)),
                levels_gained: round_half_even(levels_gained, 2),
                profession_weight: weight,
                prof_contribution,
                profession_contributions,
                hp_increase: (hp_increase > 0.0).then(|| round_half_even(hp_increase, 2)),
                hp_gain,
                recommend_rank: None,
            });
        }

        // Both orderings sort the rendered (rounded) figures the struct
        // stores; the stable sort preserves entry order on full ties.
        if target == "hp" {
            // Highest HP gain first, then lower current level (absent
            // levels last), then name.
            skills.sort_by(|a, b| {
                b.hp_gain
                    .partial_cmp(&a.hp_gain)
                    .expect("finite hpGain")
                    .then_with(|| {
                        let level =
                            |option: &SkillOption| option.current_level.unwrap_or(f64::INFINITY);
                        level(a).partial_cmp(&level(b)).expect("finite level")
                    })
                    .then_with(|| a.skill_name.cmp(b.skill_name))
            });
        } else {
            // Highest profession contribution first, then weight, then
            // name.
            skills.sort_by(|a, b| {
                b.prof_contribution
                    .partial_cmp(&a.prof_contribution)
                    .expect("finite contribution")
                    .then_with(|| b.profession_weight.cmp(&a.profession_weight))
                    .then_with(|| a.skill_name.cmp(b.skill_name))
            });
        }

        // 1-based rank over the skills relevant to the active target.
        let mut rank_counter = 0i64;
        for skill in &mut skills {
            let relevant = if target == "hp" {
                skill.hp_gain > 0.0
            } else {
                skill.profession_weight > 0
            };
            skill.recommend_rank = relevant.then(|| {
                rank_counter += 1;
                rank_counter
            });
        }

        Ok(skills)
    }

    /// Claim a mastery reward for a species whose 25 ranks are
    /// complete: a repeatable claim with no ceiling, into any
    /// mastery-eligible skill, for that skill's fixed reward.
    ///
    /// Persists in `codex_claims` with `kind='mastery'`, the rank
    /// column carrying the per-species claim sequence number (1-based),
    /// and updates the skill calibration exactly as a rank claim does
    /// (including the silent skip for an uncalibrated skill; see the
    /// module doc). `codex_progress` stays at 25.
    ///
    /// The rank-25 gate is re-checked inside the writer transaction, so
    /// a concurrent unclaim of rank 25 cannot slip a mastery claim
    /// under it; the sequence number is computed in the same
    /// transaction, so racing mastery claims serialise cleanly.
    pub async fn mastery_claim(
        &self,
        species_name: &str,
        skill_name: &str,
    ) -> Result<MasteryClaimRecord, CodexError> {
        if self.find_species(species_name).is_none() {
            return Err(CodexError::Invalid(format!(
                "Species '{species_name}' not found in game-data catalogue"
            )));
        }
        let Some(ped_value) = mastery_reward_ped(skill_name) else {
            return Err(CodexError::Invalid(format!(
                "Skill '{skill_name}' not valid for mastery"
            )));
        };

        let now = naive_to_epoch(self.clock.now());
        let species_owned = species_name.to_string();
        let skill_owned = skill_name.to_string();
        let outcome = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;

                let current_rank: i64 = tx
                    .query_row(
                        "SELECT current_rank FROM codex_progress WHERE species_name = ?",
                        rusqlite::params![&species_owned],
                        |row| row.get(0),
                    )
                    .optional()?
                    .unwrap_or(0);
                if current_rank != 25 {
                    return Ok(MasteryClaimOutcome::NotAt25 { current_rank });
                }

                let mastery_level: i64 = tx.query_row(
                    "SELECT COUNT(*) + 1 FROM codex_claims \
                     WHERE species_name = ? AND kind = 'mastery'",
                    rusqlite::params![&species_owned],
                    |row| row.get(0),
                )?;
                tx.execute(
                    "INSERT INTO codex_claims \
                     (species_name, rank, skill_name, ped_value, claimed_at, kind) \
                     VALUES (?, ?, ?, ?, ?, 'mastery')",
                    rusqlite::params![&species_owned, mastery_level, &skill_owned, ped_value, now],
                )?;
                crate::daily_rollup::refresh_days(&tx, [crate::daily_rollup::epoch_day(now)])?;

                write_codex_calibration(&tx, &skill_owned, ped_value, now)?;
                tx.commit()?;
                Ok(MasteryClaimOutcome::Done { mastery_level })
            })
            .await?;

        match outcome {
            MasteryClaimOutcome::NotAt25 { current_rank } => Err(CodexError::Invalid(format!(
                "Mastery for '{species_name}' requires rank 25 (current rank {current_rank})"
            ))),
            MasteryClaimOutcome::Done { mastery_level } => Ok(MasteryClaimRecord {
                species_name: species_name.to_string(),
                mastery_level,
                skill_name: skill_name.to_string(),
                ped_value,
            }),
        }
    }

    /// Revert a species' most recent mastery claim: delete the claim
    /// record and remove the codex-sourced calibration it wrote (the
    /// mirror of `unclaim_rank`, without a rank step: `codex_progress`
    /// stays at 25). The latest claim is the one with the highest
    /// sequence number, read and deleted inside one writer transaction,
    /// so racing unclaims serialise cleanly.
    pub async fn mastery_unclaim(
        &self,
        species_name: &str,
    ) -> Result<MasteryClaimRecord, CodexError> {
        let species_owned = species_name.to_string();
        let outcome = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;

                let claim: Option<(i64, i64, String, f64, f64)> = tx
                    .query_row(
                        "SELECT id, rank, skill_name, ped_value, claimed_at FROM codex_claims \
                         WHERE species_name = ? AND kind = 'mastery' \
                         ORDER BY rank DESC, id DESC LIMIT 1",
                        rusqlite::params![&species_owned],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, i64>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, f64>(3)?,
                                row.get::<_, f64>(4)?,
                            ))
                        },
                    )
                    .optional()?;
                let Some((id, mastery_level, skill_name, ped_value, claimed_at)) = claim else {
                    return Ok(MasteryUnclaimOutcome::NothingClaimed);
                };

                tx.execute(
                    "DELETE FROM codex_claims WHERE id = ?",
                    rusqlite::params![id],
                )?;

                // Remove the codex-sourced calibration this claim wrote,
                // matched on the instant the two inserts share; an
                // uncalibrated-skill claim (which wrote none) removes
                // nothing here.
                // id-order: tiebreak (rows already narrowed to one scanned_at).
                tx.execute(
                    "DELETE FROM skill_calibrations WHERE id = ( \
                        SELECT id FROM skill_calibrations \
                        WHERE skill_name = ? AND source = 'codex' AND scanned_at = ? \
                        ORDER BY id DESC LIMIT 1)",
                    rusqlite::params![&skill_name, claimed_at],
                )?;
                crate::daily_rollup::refresh_days(
                    &tx,
                    [crate::daily_rollup::epoch_day(claimed_at)],
                )?;

                tx.commit()?;
                Ok(MasteryUnclaimOutcome::Done {
                    mastery_level,
                    skill_name,
                    ped_value,
                })
            })
            .await?;

        match outcome {
            MasteryUnclaimOutcome::NothingClaimed => Err(CodexError::Invalid(format!(
                "No mastery claim to unclaim for '{species_name}'"
            ))),
            MasteryUnclaimOutcome::Done {
                mastery_level,
                skill_name,
                ped_value,
            } => Ok(MasteryClaimRecord {
                species_name: species_name.to_string(),
                mastery_level,
                skill_name,
                ped_value,
            }),
        }
    }

    /// Claim a meta codex reward: 1 PES into an attribute, persisted
    /// in `codex_claims` with `kind='meta'` and sentinel species and
    /// skill columns (no calibration update; no attribute curve
    /// exists).
    pub async fn meta_claim(&self, attribute_name: &str) -> Result<MetaClaimRecord, CodexError> {
        if !ATTRIBUTES.contains(&attribute_name) {
            return Err(CodexError::Invalid(format!(
                "'{attribute_name}' is not an attribute. \
                 Valid: ['Agility', 'Health', 'Intelligence', 'Psyche', 'Stamina', 'Strength']"
            )));
        }
        let now = naive_to_epoch(self.clock.now());
        let attr = attribute_name.to_string();
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO codex_claims \
                     (species_name, rank, skill_name, ped_value, claimed_at, kind, attribute_name) \
                     VALUES ('__meta__', 0, ?, ?, ?, 'meta', ?)",
                    rusqlite::params![attr, META_PED, now, attr],
                )?;
                crate::daily_rollup::refresh_days(&tx, [crate::daily_rollup::epoch_day(now)])?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(MetaClaimRecord {
            attribute_name: attribute_name.to_string(),
            ped_value: META_PED,
        })
    }

    /// The six attributes with their current calibrated levels.
    pub async fn get_meta_attributes(&self) -> Result<Vec<MetaAttribute>, CodexError> {
        let mut result = Vec::with_capacity(ATTRIBUTES.len());
        for attribute in ATTRIBUTES {
            let level = self.skill_level(attribute).await?;
            result.push(MetaAttribute {
                name: attribute,
                current_level: level.map(|level| round_half_even(level, 1)),
            });
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
        let species_owned = species_name.to_string();
        Ok(self
            .db
            .with_reader(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT current_rank FROM codex_progress WHERE species_name = ?",
                        rusqlite::params![species_owned],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                    .unwrap_or(0))
            })
            .await?)
    }

    /// The latest calibrated level for a skill, by scan instant with
    /// the row id as tiebreak: repeat claims within one instant (a
    /// mastery claim streak lands several same-skill rewards on the
    /// same timestamp) must read the newest insert, not an arbitrary
    /// tied row.
    async fn skill_level(&self, skill_name: &str) -> Result<Option<f64>, CodexError> {
        let skill_owned = skill_name.to_string();
        Ok(self
            .db
            .with_reader(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT level FROM skill_calibrations WHERE skill_name = ? \
                         ORDER BY scanned_at DESC, id DESC LIMIT 1",
                        rusqlite::params![skill_owned],
                        |row| row.get::<_, f64>(0),
                    )
                    .optional()?)
            })
            .await?)
    }
}

/// Price a claimed reward onto the skill curve inside the claim's
/// transaction: the reward's TT value buys levels at the current point
/// on the curve, appended as a codex-sourced calibration row. A skill
/// with no calibration history skips the write entirely (the silent
/// calibration skip; see the module doc).
fn write_codex_calibration(
    tx: &rusqlite::Transaction<'_>,
    skill_name: &str,
    ped_value: f64,
    now: f64,
) -> Result<(), rusqlite::Error> {
    // The id tiebreak keeps repeat claims within one instant reading
    // the newest insert (see `skill_level`).
    let current_level: Option<f64> = tx
        .query_row(
            "SELECT level FROM skill_calibrations WHERE skill_name = ? \
             ORDER BY scanned_at DESC, id DESC LIMIT 1",
            rusqlite::params![skill_name],
            |row| row.get::<_, f64>(0),
        )
        .optional()?;
    if let Some(current_level) = current_level {
        let levels_gained = levels_for_tt_value(current_level, ped_value);
        let new_level = current_level + levels_gained;
        tx.execute(
            "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
             VALUES (?, ?, 'codex', ?)",
            rusqlite::params![skill_name, new_level, now],
        )?;
    }
    Ok(())
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

// Expected values in these tests are the original implementation's
// outputs, computed by running the original Python implementation
// over byte-identical catalogue fixtures and database seeds.
#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::NaiveDateTime;
    use serde_json::json;

    use super::*;
    use crate::clock::MockClock;

    /// The wire shape of a typed result, for byte-shape assertions.
    fn to_json<T: Serialize>(value: T) -> Value {
        serde_json::to_value(value).expect("codex result serialises")
    }

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
                {"name": "Scout", "skills": [
                    {"skill": {"name": "Aim"}, "weight": 30},
                    {"skill": {"name": "Dodge"}, "weight": 15},
                ]},
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

    async fn service(dir: &Path) -> (CodexService, Db) {
        let snapshot = dir.join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        write_snapshot(&snapshot);
        let db = Db::open(&dir.join("entropia_orme.db")).await.unwrap();
        let game_data = Arc::new(GameDataStore::new(&snapshot).unwrap());
        let clock = Arc::new(MockClock::new(Some(start_instant()), 0.0));
        (CodexService::new(db.clone(), game_data, clock), db)
    }

    /// The standard calibration seed: Rifle twice (the newer scan
    /// instant wins), plus Athletics, Dodge, and Agility anchors.
    async fn seed_calibrations(db: &Db) {
        for (name, level, at) in [
            ("Rifle", 90.0, 100.0),
            ("Rifle", 100.0, 200.0),
            ("Athletics", 5.0, 150.0),
            ("Dodge", 30.0, 150.0),
            ("Agility", 32.04, 150.0),
        ] {
            db.with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                     VALUES (?1, ?2, 'scan', ?3)",
                    rusqlite::params![name, level, at],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        }
    }

    fn invalid(error: CodexError) -> String {
        match error {
            CodexError::Invalid(message) => message,
            other => panic!("expected a validation error, got: {other}"),
        }
    }

    #[tokio::test]
    async fn species_listing_dedupes_skips_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _db) = service(dir.path()).await;

        // Unranked: alphabetical (every rank ties at 0); the duplicate
        // Boar keeps its first base cost, the nameless/specieless rows
        // drop, and Ghost lists through its second (costed) entry.
        let initial = to_json(svc.get_all_species().await.unwrap());
        assert_eq!(
            initial,
            json!([
                json!({"name": "Boar", "baseCost": 37.5, "codexType": "Mob", "currentRank": 0,
                       "nextRank": 1, "nextCategory": "cat1", "nextCost": 37.5, "masteryLevel": 0}),
                json!({"name": "Ghost", "baseCost": 7.0, "codexType": null, "currentRank": 0,
                       "nextRank": 1, "nextCategory": "cat1", "nextCost": 7.0, "masteryLevel": 0}),
                json!({"name": "Looter Bird", "baseCost": 10.0, "codexType": "MobLooter",
                       "currentRank": 0, "nextRank": 1, "nextCategory": "cat1", "nextCost": 10.0, "masteryLevel": 0}),
            ])
        );

        // Ranked: rank-descending, then name; the next-rank fields
        // derive from each species' own cost table.
        svc.calibrate("Looter Bird", 5).await.unwrap();
        svc.calibrate("Boar", 2).await.unwrap();
        let ranked = to_json(svc.get_all_species().await.unwrap());
        assert_eq!(
            ranked,
            json!([
                json!({"name": "Looter Bird", "baseCost": 10.0, "codexType": "MobLooter",
                       "currentRank": 5, "nextRank": 6, "nextCategory": "cat1", "nextCost": 80.0, "masteryLevel": 0}),
                json!({"name": "Boar", "baseCost": 37.5, "codexType": "Mob", "currentRank": 2,
                       "nextRank": 3, "nextCategory": "cat2", "nextCost": 112.5, "masteryLevel": 0}),
                json!({"name": "Ghost", "baseCost": 7.0, "codexType": null, "currentRank": 0,
                       "nextRank": 1, "nextCategory": "cat1", "nextCost": 7.0, "masteryLevel": 0}),
            ])
        );

        // Rank 25 has no next rank.
        svc.calibrate("Boar", 25).await.unwrap();
        let maxed = to_json(svc.get_all_species().await.unwrap());
        assert_eq!(maxed[0]["name"], "Boar");
        assert_eq!(maxed[0]["nextRank"], Value::Null);
        assert_eq!(maxed[0]["nextCategory"], Value::Null);
        assert_eq!(maxed[0]["nextCost"], Value::Null);
    }

    #[tokio::test]
    async fn the_first_catalogue_match_decides_species_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _db) = service(dir.path()).await;

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
            to_json(
                svc.get_skill_options("Nessie", 1, &[], "profession")
                    .await
                    .unwrap(),
            ),
            json!([])
        );
    }

    #[tokio::test]
    async fn rank_breakdowns_cross_reference_claims() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;
        svc.claim_rank("Boar", 1, "Rifle").await.unwrap();
        svc.claim_rank("Boar", 2, "Anatomy").await.unwrap();

        let ranks = to_json(svc.get_species_ranks("Boar").await.unwrap().unwrap());
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
        let (svc, _db) = service(dir.path()).await;

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
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;

        let result = to_json(svc.claim_rank("Boar", 1, "Rifle").await.unwrap());
        assert_eq!(
            result,
            json!({"speciesName": "Boar", "rank": 1, "skillName": "Rifle", "pedValue": 0.1875})
        );

        let now = naive_to_epoch(start_instant());
        let claim = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT species_name, rank, skill_name, ped_value, claimed_at, kind \
                     FROM codex_claims",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )?)
            })
            .await
            .unwrap();
        assert_eq!(claim.0, "Boar");
        assert_eq!(claim.1, 1);
        assert_eq!(claim.2, "Rifle");
        assert_eq!(claim.3, 0.1875);
        assert_eq!(claim.4, now);
        assert_eq!(claim.5, "rank");

        let progress: i64 = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT current_rank FROM codex_progress WHERE species_name = 'Boar'",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(progress, 1);

        // The reward priced onto the curve from the NEWEST calibration
        // (level 100, not the older 90): 100 + levels bought by 0.1875
        // PED, the original's computed 217.745.
        let calibration = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT level, scanned_at FROM skill_calibrations WHERE source = 'codex'",
                    [],
                    |row| Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?)),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(calibration.0, 217.745);
        assert_eq!(calibration.1, now);

        // The next claim builds on the advanced rank.
        let result = to_json(svc.claim_rank("Boar", 2, "Anatomy").await.unwrap());
        assert_eq!(result["pedValue"], json!(0.375));
    }

    #[tokio::test]
    async fn an_uncalibrated_skill_claim_skips_the_calibration_write() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;

        svc.claim_rank("Boar", 1, "Rifle").await.unwrap();
        svc.claim_rank("Boar", 2, "Anatomy").await.unwrap();

        // Five seeds plus Rifle's codex row; Anatomy (no calibration
        // history) recorded its claim but wrote no calibration: the
        // reward never reaches the skill curve (see the module doc).
        let count: i64 = db
            .with_reader(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM skill_calibrations", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(count, 6);
        let claims: i64 = db
            .with_reader(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM codex_claims", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(claims, 2);
    }

    #[tokio::test]
    async fn cat4_claims_price_through_the_cat4_divisor() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _db) = service(dir.path()).await;

        svc.calibrate("Looter Bird", 4).await.unwrap();
        let result = to_json(svc.claim_rank("Looter Bird", 5, "Zoology").await.unwrap());
        assert_eq!(
            result,
            json!({"speciesName": "Looter Bird", "rank": 5, "skillName": "Zoology",
                   "pedValue": 0.06})
        );
    }

    #[tokio::test]
    async fn calibrate_bounds_and_upserts() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;

        for rank in [-1, 26] {
            let error = svc.calibrate("Boar", rank).await.unwrap_err();
            assert_eq!(invalid(error), "Rank must be 0-25");
        }

        let result = to_json(svc.calibrate("Boar", 4).await.unwrap());
        assert_eq!(result, json!({"speciesName": "Boar", "rank": 4}));
        svc.calibrate("Boar", 7).await.unwrap();
        let rows: Vec<i64> = db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT current_rank FROM codex_progress WHERE species_name = 'Boar'",
                )?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "the upsert overwrites in place");
        assert_eq!(rows[0], 7);

        // Calibration is catalogue-blind and side-effect-free.
        svc.calibrate("Nessie", 0).await.unwrap();
    }

    #[tokio::test]
    async fn meta_claims_validate_record_and_report() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;

        let error = svc.meta_claim("Luck").await.unwrap_err();
        assert_eq!(
            invalid(error),
            "'Luck' is not an attribute. \
             Valid: ['Agility', 'Health', 'Intelligence', 'Psyche', 'Stamina', 'Strength']"
        );

        let result = to_json(svc.meta_claim("Agility").await.unwrap());
        assert_eq!(result, json!({"attributeName": "Agility", "pedValue": 1.0}));
        let row = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT species_name, rank, skill_name, ped_value, kind, attribute_name \
                     FROM codex_claims WHERE kind = 'meta'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )?)
            })
            .await
            .unwrap();
        assert_eq!(row.0, "__meta__");
        assert_eq!(row.1, 0);
        assert_eq!(row.2, "Agility");
        assert_eq!(row.3, 1.0);
        assert_eq!(row.5, "Agility");

        // The six attributes in sorted order, levels from the latest
        // calibration rounded to one decimal (32.04 -> 32.0).
        let attributes = to_json(svc.get_meta_attributes().await.unwrap());
        assert_eq!(
            attributes,
            json!([
                json!({"name": "Agility", "currentLevel": 32.0}),
                json!({"name": "Health", "currentLevel": null}),
                json!({"name": "Intelligence", "currentLevel": null}),
                json!({"name": "Psyche", "currentLevel": null}),
                json!({"name": "Stamina", "currentLevel": null}),
                json!({"name": "Strength", "currentLevel": null}),
            ])
        );
    }

    #[tokio::test]
    async fn the_final_rank_claims_at_the_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _db) = service(dir.path()).await;

        // Rank 25 itself is claimable; the max-rank guard rejects only
        // beyond the table. Hand-computed reward: multiplier 100 x
        // base 37.5 = 3750 kill cost, cat3 divisor 640 -> 5.859375 ->
        // 5.8594 at four places.
        svc.calibrate("Boar", 24).await.unwrap();
        let result = to_json(svc.claim_rank("Boar", 25, "Evade").await.unwrap());
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
            CodexError::Rollup(crate::db::DbError::CoreClosed).to_string(),
            crate::db::DbError::CoreClosed.to_string()
        );
    }

    #[tokio::test]
    async fn profession_options_rank_by_contribution() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;
        // Rifle advances to 217.745 through the claim, matching the
        // original's fixture sequence.
        svc.claim_rank("Boar", 1, "Rifle").await.unwrap();

        let options = to_json(
            svc.get_skill_options("Boar", 1, &["Sniper".to_string()], "profession")
                .await
                .unwrap(),
        );
        assert_eq!(options.as_array().unwrap().len(), 15);

        // Weighted skills lead, ranked by contribution computed from
        // the UNROUNDED levels (0.28749, not 0.2875 of the displayed
        // 143.75); the zero-contribution tail keeps category order
        // (the stable sort), with recommendRank withheld.
        assert_eq!(
            options[0],
            json!({"skillName": "Rifle", "category": "cat1", "rewardPed": 0.1875,
                   "currentLevel": 217.7, "levelsGained": 93.75, "professionWeight": 50,
                   "profContribution": 0.46875,
                   "professionContributions": [{"profession": "Sniper", "profContribution": 0.46875}],
                   "hpIncrease": null, "hpGain": 0.0,
                   "recommendRank": 1})
        );
        assert_eq!(
            options[1],
            json!({"skillName": "Aim", "category": "cat1", "rewardPed": 0.1875,
                   "currentLevel": null, "levelsGained": 143.75, "professionWeight": 20,
                   "profContribution": 0.28749,
                   "professionContributions": [{"profession": "Sniper", "profContribution": 0.28749}],
                   "hpIncrease": null, "hpGain": 0.0,
                   "recommendRank": 2})
        );
        assert_eq!(
            options[3],
            json!({"skillName": "Athletics", "category": "cat1", "rewardPed": 0.1875,
                   "currentLevel": 5.0, "levelsGained": 145.75, "professionWeight": 0,
                   "profContribution": 0.0, "professionContributions": [],
                   "hpIncrease": 20.0, "hpGain": 7.28725,
                   "recommendRank": null})
        );
        let names: Vec<&str> = options
            .as_array()
            .unwrap()
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
        assert!(options.as_array().unwrap()[2..]
            .iter()
            .all(|option| option["recommendRank"] == Value::Null));
    }

    #[tokio::test]
    async fn hp_options_sort_by_gain_then_level_then_name() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;

        // A MobLooter rank 5 offers cat3 plus the cat4 bonus skills.
        let options = to_json(
            svc.get_skill_options("Looter Bird", 5, &[], "hp")
                .await
                .unwrap(),
        );
        assert_eq!(options.as_array().unwrap().len(), 19);

        assert_eq!(
            options[0],
            json!({"skillName": "Zoology", "category": "cat4", "rewardPed": 0.06,
                   "currentLevel": null, "levelsGained": 44.99, "professionWeight": 0,
                   "profContribution": 0.0, "professionContributions": [],
                   "hpIncrease": 5.5, "hpGain": 8.180909,
                   "recommendRank": 1})
        );
        assert_eq!(
            options[1],
            json!({"skillName": "Dodge", "category": "cat3", "rewardPed": 0.0938,
                   "currentLevel": 30.0, "levelsGained": 78.38, "professionWeight": 0,
                   "profContribution": 0.0, "professionContributions": [],
                   "hpIncrease": 12.0, "hpGain": 6.53125,
                   "recommendRank": 2})
        );

        // The zero-gain tail interleaves cat3 and cat4 alphabetically
        // (every key ties, so the name decides).
        let names: Vec<&str> = options
            .as_array()
            .unwrap()
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
        assert!(options.as_array().unwrap()[2..]
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
            let (svc, db) = service(dir.path()).await;
            seed_calibrations(&db).await;

            let (a, b) = tokio::join!(
                svc.claim_rank("Boar", 1, "Rifle"),
                svc.claim_rank("Boar", 1, "Rifle"),
            );
            assert!(
                a.is_ok() ^ b.is_ok(),
                "exactly one claim must win: a={a:?} b={b:?}"
            );

            let claims: i64 = db
                .with_reader(|conn| {
                    Ok(conn.query_row(
                        "SELECT COUNT(*) FROM codex_claims WHERE species_name = 'Boar' AND rank = 1",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?)
                })
                .await
                .unwrap();
            assert_eq!(claims, 1, "exactly one claim row may be recorded");

            let progress: i64 = db
                .with_reader(|conn| {
                    Ok(conn.query_row(
                        "SELECT current_rank FROM codex_progress WHERE species_name = 'Boar'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?)
                })
                .await
                .unwrap();
            assert_eq!(progress, 1, "progress advances exactly once");
        }
    }

    #[tokio::test]
    async fn unclaim_reverts_progress_claim_and_calibration() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;

        // A claim advances to rank 1, records the claim, and writes a
        // codex calibration on top of Rifle's newest level (100).
        svc.claim_rank("Boar", 1, "Rifle").await.unwrap();
        assert_eq!(svc.current_rank("Boar").await.unwrap(), 1);
        assert_eq!(svc.skill_level("Rifle").await.unwrap(), Some(217.745));

        let reverted = to_json(svc.unclaim_rank("Boar").await.unwrap());
        assert_eq!(
            reverted,
            json!({"speciesName": "Boar", "rank": 1, "skillName": "Rifle", "pedValue": 0.1875})
        );

        // Rank steps back, the claim row is gone, and the codex
        // calibration is removed so Rifle reverts to its scanned 100;
        // the five seed rows are untouched.
        assert_eq!(svc.current_rank("Boar").await.unwrap(), 0);
        let claims: i64 = db
            .with_reader(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM codex_claims", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(claims, 0);
        let codex_rows: i64 = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM skill_calibrations WHERE source = 'codex'",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(codex_rows, 0);
        let total: i64 = db
            .with_reader(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM skill_calibrations", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(total, 5, "the scan-seeded calibrations are untouched");
        assert_eq!(svc.skill_level("Rifle").await.unwrap(), Some(100.0));
    }

    #[tokio::test]
    async fn claim_and_unclaim_reland_the_claim_days_rollup() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;

        svc.claim_rank("Boar", 1, "Rifle").await.unwrap();

        // Two days later the claim day is behind the heal watermark.
        let claim_epoch = crate::time::naive_to_epoch(start_instant());
        let claim_day = crate::daily_rollup::epoch_day(claim_epoch);
        svc.db()
            .with_writer(move |conn| {
                crate::daily_rollup::heal_rollups(conn, claim_epoch + 2.0 * 86_400.0)
            })
            .await
            .unwrap();
        let day = claim_day.clone();
        let codex_pes: Option<f64> = db
            .with_reader(move |conn| {
                Ok(conn.query_row(
                    "SELECT codex_pes FROM daily_rollups WHERE day = ?1",
                    rusqlite::params![day],
                    |row| row.get::<_, Option<f64>>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(codex_pes, Some(0.1875));

        // The unclaim deletes the now-historical claim and relands its
        // day inside the same transaction.
        svc.unclaim_rank("Boar").await.unwrap();
        let day = claim_day.clone();
        let codex_pes: Option<f64> = db
            .with_reader(move |conn| {
                Ok(conn.query_row(
                    "SELECT codex_pes FROM daily_rollups WHERE day = ?1",
                    rusqlite::params![day],
                    |row| row.get::<_, Option<f64>>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(codex_pes, None, "no claims remain on the day");
    }

    #[tokio::test]
    async fn unclaim_of_an_uncalibrated_skill_claim_removes_only_the_claim() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;

        // Anatomy has no calibration history, so its claim wrote none
        // (the documented silent skip); unclaiming it must not touch
        // Rifle's codex calibration from the earlier rank.
        svc.claim_rank("Boar", 1, "Rifle").await.unwrap();
        svc.claim_rank("Boar", 2, "Anatomy").await.unwrap();

        svc.unclaim_rank("Boar").await.unwrap();

        assert_eq!(svc.current_rank("Boar").await.unwrap(), 1);
        let claims: i64 = db
            .with_reader(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM codex_claims", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(claims, 1, "only the Anatomy claim is removed");
        let codex_rows: i64 = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM skill_calibrations WHERE source = 'codex'",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(codex_rows, 1, "Rifle's codex calibration survives");
    }

    #[tokio::test]
    async fn unclaim_requires_a_claimed_latest_rank() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _db) = service(dir.path()).await;

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
            let (svc, db) = service(dir.path()).await;
            seed_calibrations(&db).await;
            svc.claim_rank("Boar", 1, "Rifle").await.unwrap();

            let (a, b) = tokio::join!(svc.unclaim_rank("Boar"), svc.unclaim_rank("Boar"));
            assert!(
                a.is_ok() ^ b.is_ok(),
                "exactly one unclaim must win: a={a:?} b={b:?}"
            );

            let claims: i64 = db
                .with_reader(|conn| {
                    Ok(conn.query_row(
                        "SELECT COUNT(*) FROM codex_claims WHERE species_name = 'Boar' AND rank = 1",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?)
                })
                .await
                .unwrap();
            assert_eq!(claims, 0, "the single claim is reverted exactly once");

            let progress: i64 = db
                .with_reader(|conn| {
                    Ok(conn.query_row(
                        "SELECT current_rank FROM codex_progress WHERE species_name = 'Boar'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?)
                })
                .await
                .unwrap();
            assert_eq!(progress, 0, "rank steps back exactly once");
        }
    }

    #[tokio::test]
    async fn several_professions_rank_by_summed_weights() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;

        // A family of professions targeted together sums weights per
        // skill (contribution is linear in weight, so this equals the
        // sum of per-profession contributions): rank 1 is cat1, where
        // Sniper carries Rifle 50 / Aim 20 and Scout adds Aim 30.
        let options = svc
            .get_skill_options(
                "Boar",
                1,
                &["Sniper".to_string(), "Scout".to_string()],
                "profession",
            )
            .await
            .unwrap();
        let weight = |name: &str| {
            options
                .iter()
                .find(|option| option.skill_name == name)
                .unwrap_or_else(|| panic!("{name} missing"))
                .profession_weight
        };
        assert_eq!(weight("Aim"), 50);
        assert_eq!(weight("Rifle"), 50);
        assert_eq!(weight("Anatomy"), 0);

        // The per-profession split carries one entry per weighted
        // requested profession, in request order, each computed from
        // the same unrounded levels as the summed figure.
        let aim = options
            .iter()
            .find(|option| option.skill_name == "Aim")
            .unwrap();
        assert_eq!(
            to_json(&aim.profession_contributions),
            json!([
                {"profession": "Sniper", "profContribution": 0.28749},
                {"profession": "Scout", "profContribution": 0.431235},
            ])
        );
        let rifle_only = options
            .iter()
            .find(|option| option.skill_name == "Rifle")
            .unwrap();
        assert_eq!(rifle_only.profession_contributions.len(), 1);
        assert_eq!(rifle_only.profession_contributions[0].profession, "Sniper");

        // Aim outranks Rifle at equal weight: Rifle's calibrated level
        // 100 buys far fewer levels per PED than uncalibrated Aim.
        let rank_of = |name: &str| {
            options
                .iter()
                .find(|option| option.skill_name == name)
                .unwrap()
                .recommend_rank
        };
        assert_eq!(rank_of("Aim"), Some(1));
        assert_eq!(rank_of("Rifle"), Some(2));

        // A duplicated name never double-counts its weights.
        let deduped = svc
            .get_skill_options(
                "Boar",
                1,
                &["Sniper".to_string(), "Sniper".to_string()],
                "profession",
            )
            .await
            .unwrap();
        let rifle = deduped
            .iter()
            .find(|option| option.skill_name == "Rifle")
            .unwrap();
        assert_eq!(rifle.profession_weight, 50);
    }

    #[tokio::test]
    async fn mastery_history_blocks_the_rank_25_unclaim() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _db) = service(dir.path()).await;

        // A claimed rank 25 with mastery claims on top only reverts
        // once the mastery history is undone: its calibration is what
        // the mastery rewards priced from.
        svc.calibrate("Boar", 24).await.unwrap();
        svc.claim_rank("Boar", 25, "Evade").await.unwrap();
        svc.mastery_claim("Boar", "Rifle").await.unwrap();

        let error = svc.unclaim_rank("Boar").await.unwrap_err();
        assert_eq!(
            invalid(error),
            "Undo the 1 mastery claim(s) for 'Boar' before unclaiming rank 25"
        );
        assert_eq!(svc.current_rank("Boar").await.unwrap(), 25);

        svc.mastery_unclaim("Boar").await.unwrap();
        let reverted = svc.unclaim_rank("Boar").await.unwrap();
        assert_eq!(reverted.rank, 25);
        assert_eq!(svc.current_rank("Boar").await.unwrap(), 24);
    }

    /// Build the service over a Python-lineage version-33 database whose
    /// `codex_claims` predates the mastery columns (`kind`/`attribute_name`):
    /// exactly the drifted shape adoption used to stamp as-is, leaving every
    /// codex read to fail on the missing `kind` column. `Db::open` heals the
    /// drift, so the service must serve reads and record mastery/meta claims.
    async fn legacy_service(dir: &Path) -> (CodexService, Db) {
        let snapshot = dir.join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        write_snapshot(&snapshot);
        let path = dir.join("entropia_orme.db");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(include_str!("../migrations/0001_schema_baseline.sql"))
                .unwrap();
            connection
                .execute_batch(crate::db::LEGACY_CODEX_CLAIMS_DDL)
                .unwrap();
        }
        let db = Db::open(&path).await.unwrap();
        let game_data = Arc::new(GameDataStore::new(&snapshot).unwrap());
        let clock = Arc::new(MockClock::new(Some(start_instant()), 0.0));
        (CodexService::new(db.clone(), game_data, clock), db)
    }

    #[tokio::test]
    async fn adopted_legacy_database_serves_codex_reads_and_mastery_claims() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _db) = legacy_service(dir.path()).await;

        // The species read that failed with `no such column: kind` on the
        // drifted database now succeeds after the open-time heal.
        let species = svc.get_all_species().await.unwrap();
        assert!(species.iter().any(|s| s.name == "Boar"));

        // The mastery and meta writers target the healed columns; both record.
        svc.calibrate("Boar", 24).await.unwrap();
        svc.claim_rank("Boar", 25, "Evade").await.unwrap();
        let mastery = svc.mastery_claim("Boar", "Rifle").await.unwrap();
        assert_eq!(mastery.mastery_level, 1);
        let meta = svc.meta_claim("Agility").await.unwrap();
        assert_eq!(meta.attribute_name, "Agility");

        // The mastery read reflects the recorded claim.
        let after = svc.get_all_species().await.unwrap();
        let boar = after.iter().find(|s| s.name == "Boar").unwrap();
        assert_eq!(boar.mastery_level, 1);
    }

    #[tokio::test]
    async fn repeat_same_skill_mastery_claims_price_from_the_newest_calibration() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;
        svc.calibrate("Boar", 25).await.unwrap();

        // Three same-skill claims share one clock instant; each must
        // price from the previous claim's level (the id tiebreak on the
        // calibration read), not an arbitrary tied row.
        let mut expected = 100.0;
        for _ in 0..3 {
            svc.mastery_claim("Boar", "Rifle").await.unwrap();
            expected += levels_for_tt_value(expected, 25.0);
        }
        let top: f64 = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT level FROM skill_calibrations WHERE skill_name = 'Rifle' \
                     ORDER BY scanned_at DESC, id DESC LIMIT 1",
                    [],
                    |row| row.get::<_, f64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(top, expected);

        // Unclaiming steps back through the same ladder: the newest
        // calibration is removed and the next claim re-prices from the
        // level beneath it.
        svc.mastery_unclaim("Boar").await.unwrap();
        let renewed = svc.mastery_claim("Boar", "Rifle").await.unwrap();
        assert_eq!(renewed.mastery_level, 3);
        let count: i64 = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM skill_calibrations WHERE source = 'codex'",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn mastery_claims_gate_on_species_rank_and_skill() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _db) = service(dir.path()).await;

        let error = svc.mastery_claim("Nessie", "Rifle").await.unwrap_err();
        assert_eq!(
            invalid(error),
            "Species 'Nessie' not found in game-data catalogue"
        );

        // Skill eligibility refuses cat4 and unknown skills before any
        // rank check (the reward has no defined value for them).
        let error = svc.mastery_claim("Boar", "Zoology").await.unwrap_err();
        assert_eq!(invalid(error), "Skill 'Zoology' not valid for mastery");
        let error = svc
            .mastery_claim("Boar", "Fishing Rod Technology")
            .await
            .unwrap_err();
        assert_eq!(
            invalid(error),
            "Skill 'Fishing Rod Technology' not valid for mastery"
        );

        // The rank-25 gate, from unranked and from a mid-codex rank.
        let error = svc.mastery_claim("Boar", "Rifle").await.unwrap_err();
        assert_eq!(
            invalid(error),
            "Mastery for 'Boar' requires rank 25 (current rank 0)"
        );
        svc.calibrate("Boar", 24).await.unwrap();
        let error = svc.mastery_claim("Boar", "Rifle").await.unwrap_err();
        assert_eq!(
            invalid(error),
            "Mastery for 'Boar' requires rank 25 (current rank 24)"
        );
    }

    #[tokio::test]
    async fn mastery_claims_sequence_calibrate_and_surface_the_tally() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;
        svc.calibrate("Boar", 25).await.unwrap();

        let first = to_json(svc.mastery_claim("Boar", "Rifle").await.unwrap());
        assert_eq!(
            first,
            json!({"speciesName": "Boar", "masteryLevel": 1, "skillName": "Rifle",
                   "pedValue": 25.0})
        );

        // The claim row carries the mastery kind and the sequence in
        // the rank column; progress stays pinned at 25.
        let now = naive_to_epoch(start_instant());
        let claim = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT rank, skill_name, ped_value, claimed_at, kind FROM codex_claims",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, f64>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )?)
            })
            .await
            .unwrap();
        assert_eq!(
            claim,
            (1, "Rifle".to_string(), 25.0, now, "mastery".to_string())
        );
        assert_eq!(svc.current_rank("Boar").await.unwrap(), 25);

        // The reward priced onto the curve from the newest calibration
        // (level 100), exactly as a rank claim does.
        let calibration: (f64, f64) = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT level, scanned_at FROM skill_calibrations WHERE source = 'codex'",
                    [],
                    |row| Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?)),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(calibration.0, 100.0 + levels_for_tt_value(100.0, 25.0));
        assert_eq!(calibration.1, now);

        // Repeat claims sequence 2, 3, ... with per-category values.
        let second = to_json(svc.mastery_claim("Boar", "Courage").await.unwrap());
        assert_eq!(second["masteryLevel"], json!(2));
        assert_eq!(second["pedValue"], json!(15.625));
        let third = to_json(svc.mastery_claim("Boar", "Dodge").await.unwrap());
        assert_eq!(third["masteryLevel"], json!(3));
        assert_eq!(third["pedValue"], json!(7.8125));

        // The tally surfaces on the listing and the rank breakdown, and
        // the mastery rows never pollute the 1-25 claim overlay.
        let listing = to_json(svc.get_all_species().await.unwrap());
        assert_eq!(listing[0]["name"], "Boar");
        assert_eq!(listing[0]["masteryLevel"], json!(3));
        let ranks = svc.get_species_ranks("Boar").await.unwrap().unwrap();
        assert_eq!(ranks.mastery_level, 3);
        assert_eq!(
            to_json(&ranks.mastery_claims),
            json!([
                {"masteryLevel": 1, "skillName": "Rifle", "pedValue": 25.0},
                {"masteryLevel": 2, "skillName": "Courage", "pedValue": 15.625},
                {"masteryLevel": 3, "skillName": "Dodge", "pedValue": 7.8125},
            ])
        );
        assert!(
            ranks.ranks.iter().all(|rank| !rank.claimed),
            "mastery claims must not read as rank claims"
        );
    }

    #[tokio::test]
    async fn an_uncalibrated_skill_mastery_claim_skips_the_calibration_write() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;
        svc.calibrate("Boar", 25).await.unwrap();

        // Anatomy has no calibration history: the claim records but the
        // reward never reaches the skill curve (see the module doc).
        svc.mastery_claim("Boar", "Anatomy").await.unwrap();
        let codex_rows: i64 = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM skill_calibrations WHERE source = 'codex'",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(codex_rows, 0);
    }

    #[tokio::test]
    async fn mastery_unclaims_revert_the_latest_claim_and_its_calibration() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;
        svc.calibrate("Boar", 25).await.unwrap();

        // Nothing to unclaim yet.
        let error = svc.mastery_unclaim("Boar").await.unwrap_err();
        assert_eq!(invalid(error), "No mastery claim to unclaim for 'Boar'");

        svc.mastery_claim("Boar", "Rifle").await.unwrap();
        svc.mastery_claim("Boar", "Dodge").await.unwrap();

        // The latest claim (Dodge, sequence 2) reverts first, removing
        // its calibration but not Rifle's; progress stays at 25.
        let reverted = to_json(svc.mastery_unclaim("Boar").await.unwrap());
        assert_eq!(
            reverted,
            json!({"speciesName": "Boar", "masteryLevel": 2, "skillName": "Dodge",
                   "pedValue": 7.8125})
        );
        assert_eq!(svc.current_rank("Boar").await.unwrap(), 25);
        let remaining: Vec<String> = db
            .with_reader(|conn| {
                let mut stmt = conn
                    .prepare("SELECT skill_name FROM skill_calibrations WHERE source = 'codex'")?;
                let mut rows = stmt.query([])?;
                let mut names = Vec::new();
                while let Some(row) = rows.next()? {
                    names.push(row.get::<_, String>(0)?);
                }
                Ok(names)
            })
            .await
            .unwrap();
        assert_eq!(remaining, vec!["Rifle".to_string()]);

        // The next claim reuses the freed sequence number.
        let renewed = to_json(svc.mastery_claim("Boar", "Courage").await.unwrap());
        assert_eq!(renewed["masteryLevel"], json!(2));
    }

    #[tokio::test]
    async fn mastery_claims_land_in_the_daily_rollup() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        svc.calibrate("Boar", 25).await.unwrap();

        svc.mastery_claim("Boar", "Rifle").await.unwrap();

        // Two days later the claim day is behind the heal watermark.
        let claim_epoch = naive_to_epoch(start_instant());
        let claim_day = crate::daily_rollup::epoch_day(claim_epoch);
        svc.db()
            .with_writer(move |conn| {
                crate::daily_rollup::heal_rollups(conn, claim_epoch + 2.0 * 86_400.0)
            })
            .await
            .unwrap();
        let day = claim_day.clone();
        let codex_pes: Option<f64> = db
            .with_reader(move |conn| {
                Ok(conn.query_row(
                    "SELECT codex_pes FROM daily_rollups WHERE day = ?1",
                    rusqlite::params![day],
                    |row| row.get::<_, Option<f64>>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(codex_pes, Some(25.0));

        // The unclaim deletes the claim and relands its day inside the
        // same transaction.
        svc.mastery_unclaim("Boar").await.unwrap();
        let codex_pes: Option<f64> = db
            .with_reader(move |conn| {
                Ok(conn.query_row(
                    "SELECT codex_pes FROM daily_rollups WHERE day = ?1",
                    rusqlite::params![claim_day],
                    |row| row.get::<_, Option<f64>>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(codex_pes, None, "no claims remain on the day");
    }

    #[tokio::test]
    async fn mastery_options_span_the_eligible_skills_and_rank_like_ranks_do() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, db) = service(dir.path()).await;
        seed_calibrations(&db).await;

        let options = svc
            .get_mastery_skill_options(&["Sniper".to_string()], "profession")
            .await
            .unwrap();

        // Every cat1-cat3 skill exactly once, no cat4.
        assert_eq!(options.len(), 36);
        assert!(options.iter().all(|option| option.category != "cat4"));

        // Fixed per-category rewards, independent of any species.
        let reward = |name: &str| {
            options
                .iter()
                .find(|option| option.skill_name == name)
                .unwrap_or_else(|| panic!("{name} missing"))
                .reward_ped
        };
        assert_eq!(reward("Aim"), 25.0);
        assert_eq!(reward("Courage"), 15.625);
        assert_eq!(reward("Evade"), 7.8125);

        // Recommendation ranks assign to the profession-weighted skills
        // only (Rifle and Aim carry Sniper weights; Zoology's weight is
        // out of reach as cat4), exactly as the per-rank options do.
        let ranked: Vec<&str> = options
            .iter()
            .filter(|option| option.recommend_rank.is_some())
            .map(|option| option.skill_name)
            .collect();
        assert_eq!(ranked.len(), 2);
        assert!(ranked.contains(&"Rifle") && ranked.contains(&"Aim"));
        let first = options
            .iter()
            .find(|option| option.recommend_rank == Some(1))
            .unwrap();
        assert!(
            first.prof_contribution
                >= options
                    .iter()
                    .find(|option| option.recommend_rank == Some(2))
                    .unwrap()
                    .prof_contribution
        );
    }
}
