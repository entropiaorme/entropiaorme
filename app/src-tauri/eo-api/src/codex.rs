//! The codex family: the species listing and per-species rank breakdown,
//! the skill-recommendation read, the six meta attributes, the
//! meta-target ranking, and the
//! claim / unclaim / calibrate / meta-claim writes.
//!
//! The computation stays in `eo_services::codex::CodexService`, which
//! returns typed records; the facade maps them field by field onto the
//! declared DTOs, so the wire shape is single-sourced in Rust and the
//! mapping is compiler-checked. The response DTOs' field order is the
//! golden-pinned wire order.
//!
//! Contract lineage (ADR-0017/0019): several transport-era behaviours
//! retired at the typed-command crossing. The conditional-GET (ETag) contract retires with
//! the transport (the reads answer their body directly). The recommend
//! route's rank 422 envelope becomes a typed `bad_request` (the bound is
//! now checked on the `i64` argument, not a pydantic query model), its
//! `target` vocabulary is a closed enum (the out-of-vocabulary 422 is
//! unrepresentable), and the calibrate route's surrogate-codec 400 plus
//! the body-taint / beyond-`i64` 422/500 ceremony become unrepresentable
//! over the typed command (a `String` argument cannot carry a surrogate
//! and an `i64` rank cannot overflow the parse).

use eo_services::codex::{self, CodexError, CodexService};
use eo_services::skill_tracker::SUPPRESS_TIMEOUT_SECONDS;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Nullable;
use crate::{Api, ApiError};

// ── Request arguments ───────────────────────────────────────────────

/// What a codex recommendation ranks by. A closed vocabulary: the
/// bindings expose only these two, so an out-of-vocabulary `target` is
/// unrepresentable rather than validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CodexRecommendTarget {
    Profession,
    Hp,
}

impl CodexRecommendTarget {
    fn as_str(self) -> &'static str {
        match self {
            CodexRecommendTarget::Profession => "profession",
            CodexRecommendTarget::Hp => "hp",
        }
    }
}

// ── Response DTOs ───────────────────────────────────────────────────

/// One species in the codex listing: its base cost, the player's current
/// rank, and the next rank's derived category and cost (all `null` once
/// rank 25 is reached).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexSpecies {
    pub name: String,
    pub base_cost: f64,
    pub codex_type: Nullable<String>,
    pub current_rank: i64,
    pub next_rank: Nullable<i64>,
    pub next_category: Nullable<String>,
    pub next_cost: Nullable<f64>,
    pub mastery_level: i64,
}

/// One rank in a species' breakdown: the derived cost / reward / category
/// fields, plus the player's claim state for that rank. Field order is
/// the wire order (the breakdown's own fields, then the claim overlay).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexRank {
    pub rank: i64,
    pub category: String,
    pub cost: f64,
    pub reward_ped: f64,
    pub cat4_bonus: bool,
    pub cat4_reward_ped: Option<f64>,
    pub skills: Vec<String>,
    pub cat4_skills: Vec<String>,
    pub claimed: bool,
    pub claimed_skill: Nullable<String>,
    pub claimed_ped: Nullable<f64>,
    pub is_next: bool,
}

/// One recorded mastery claim in a species' breakdown, in claim order
/// (`mastery_level` is the 1-based per-species sequence number).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexMasteryClaim {
    pub mastery_level: i64,
    pub skill_name: String,
    pub ped_value: f64,
}

/// A species' full 25-rank breakdown, cross-referenced with the player's
/// claims and current rank.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexSpeciesRanks {
    pub species_name: String,
    pub base_cost: f64,
    pub codex_type: Nullable<String>,
    pub current_rank: i64,
    pub mastery_level: i64,
    pub mastery_claims: Vec<CodexMasteryClaim>,
    pub ranks: Vec<CodexRank>,
}

/// One requested profession's share of a skill option's contribution
/// (professions with no weight on the skill are omitted).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexProfessionContribution {
    pub profession: String,
    pub prof_contribution: f64,
}

/// One skill option in a rank recommendation: the reward it grants, the
/// levels that buys at the current point on the curve, and the profession
/// / HP contribution used to rank the list. The weight and contribution
/// sum over the requested professions; `profession_contributions` carries
/// the per-profession split.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexSkillOption {
    pub skill_name: String,
    pub category: String,
    pub reward_ped: f64,
    pub current_level: Nullable<f64>,
    pub levels_gained: f64,
    pub profession_weight: i64,
    pub prof_contribution: f64,
    pub profession_contributions: Vec<CodexProfessionContribution>,
    pub hp_increase: Nullable<f64>,
    pub hp_gain: f64,
    pub recommend_rank: Nullable<i64>,
}

/// One meta attribute with its current calibrated level.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexMetaAttribute {
    pub name: String,
    pub current_level: Nullable<f64>,
}

/// One species' next meta reward: the breakpoint it reaches next, the
/// codex cost of the ranks left before it, and that cost as estimated PED
/// cycled (a planning estimate, never an accounting figure). `planets` is
/// sorted and empty when the catalogue names none.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexMetaTarget {
    pub species_name: String,
    pub planets: Vec<String>,
    pub current_rank: i64,
    pub target_rank: i64,
    pub cost: f64,
    pub estimated_ped: f64,
}

/// The meta-target ranking: every species short of rank 25, cheapest next
/// meta first, with the metas the recorded ranks have earned against the
/// cap and the rate the PED estimates use.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexMetaTargets {
    pub metas_earned: i64,
    pub meta_cap: i64,
    pub ped_per_cost: f64,
    pub targets: Vec<CodexMetaTarget>,
}

/// The record a rank claim (or its reversal) returns.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexClaimResult {
    pub species_name: String,
    pub rank: i64,
    pub skill_name: String,
    pub ped_value: f64,
}

/// The record a manual rank calibration returns.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexCalibrateResult {
    pub species_name: String,
    pub rank: i64,
}

/// The record a meta claim returns.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexMetaClaimResult {
    pub attribute_name: String,
    pub ped_value: f64,
}

/// The record a mastery claim (or its reversal) returns.
/// `mastery_level` is the per-species claim sequence number (the Nth
/// mastery claim for that species).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexMasteryClaimResult {
    pub species_name: String,
    pub mastery_level: i64,
    pub skill_name: String,
    pub ped_value: f64,
}

// ── Facade methods ──────────────────────────────────────────────────

impl Api {
    /// All mob species carrying a codex base cost, cross-referenced with
    /// the player's rank, sorted rank-descending then name-ascending.
    pub async fn codex_species(&self) -> Result<Vec<CodexSpecies>, ApiError> {
        let species = self
            .codex
            .get_all_species()
            .await
            .map_err(ApiError::internal("codex species read"))?;
        Ok(species.into_iter().map(species_dto).collect())
    }

    /// The 25-rank breakdown for a species, cross-referenced with claims.
    /// A species absent from the catalogue is a not-found (the HTTP
    /// route's 404).
    pub async fn codex_species_ranks(
        &self,
        species_name: &str,
    ) -> Result<CodexSpeciesRanks, ApiError> {
        let ranks = self
            .codex
            .get_species_ranks(species_name)
            .await
            .map_err(ApiError::internal("codex species ranks read"))?;
        let Some(ranks) = ranks else {
            return Err(ApiError::not_found(format!(
                "Species '{species_name}' not found"
            )));
        };
        Ok(species_ranks_dto(ranks))
    }

    /// The skill options for a rank, ranked by profession contribution or
    /// HP gain. An empty list when the species is not in the catalogue
    /// (the pinned soft-miss shape). Several professions rank by summed
    /// weights (a profession family targeted together). The rank is
    /// bound to the codex table's 1..=25 domain; an out-of-domain rank
    /// is a typed `bad_request`.
    pub async fn codex_recommend(
        &self,
        species_name: &str,
        rank: i64,
        professions: &[String],
        target: CodexRecommendTarget,
    ) -> Result<Vec<CodexSkillOption>, ApiError> {
        if !(1..=25).contains(&rank) {
            return Err(ApiError::bad_request("rank must be between 1 and 25"));
        }
        let options = self
            .codex
            .get_skill_options(species_name, rank, professions, target.as_str())
            .await
            .map_err(ApiError::internal("codex recommend read"))?;
        Ok(options.into_iter().map(skill_option_dto).collect())
    }

    /// The six meta attributes with their current calibrated levels.
    pub async fn codex_meta_attributes(&self) -> Result<Vec<CodexMetaAttribute>, ApiError> {
        let attributes = self
            .codex
            .get_meta_attributes()
            .await
            .map_err(ApiError::internal("codex meta attributes read"))?;
        Ok(attributes
            .into_iter()
            .map(|attribute| CodexMetaAttribute {
                name: attribute.name.to_string(),
                current_level: attribute.current_level.into(),
            })
            .collect())
    }

    /// Every species short of rank 25, ranked by the codex cost of its
    /// next meta reward, with the earned-meta tally.
    pub async fn codex_meta_targets(&self) -> Result<CodexMetaTargets, ApiError> {
        let result = self
            .codex
            .get_meta_targets()
            .await
            .map_err(ApiError::internal("codex meta targets read"))?;
        Ok(CodexMetaTargets {
            metas_earned: result.metas_earned,
            meta_cap: result.meta_cap,
            ped_per_cost: result.ped_per_cost,
            targets: result
                .targets
                .into_iter()
                .map(|target| CodexMetaTarget {
                    species_name: target.species_name,
                    planets: target.planets,
                    current_rank: target.current_rank,
                    target_rank: target.target_rank,
                    cost: target.cost,
                    estimated_ped: target.estimated_ped,
                })
                .collect(),
        })
    }

    /// Set a species' codex rank directly (manual calibration, no side
    /// effects). An out-of-domain rank is the service's `bad_request`.
    pub async fn codex_calibrate(
        &self,
        species_name: &str,
        rank: i64,
    ) -> Result<CodexCalibrateResult, ApiError> {
        let result = self
            .codex
            .calibrate(species_name, rank)
            .await
            .map_err(codex_write_error("codex calibrate"))?;
        Ok(CodexCalibrateResult {
            species_name: result.species_name,
            rank: result.rank,
        })
    }

    /// Claim a codex rank reward. On success, an active session suppresses
    /// the claimed skill's next gain from dedup (`suppress_next`), only
    /// after the claim succeeds.
    pub async fn codex_claim(
        &self,
        species_name: &str,
        rank: i64,
        skill_name: &str,
    ) -> Result<CodexClaimResult, ApiError> {
        let result = self
            .codex
            .claim_rank(species_name, rank, skill_name)
            .await
            .map_err(codex_write_error("codex claim"))?;
        if self.tracker.is_tracking() {
            self.skill_tracker
                .suppress_next(skill_name, SUPPRESS_TIMEOUT_SECONDS);
        }
        Ok(claim_dto(result))
    }

    /// Revert a species' most recent rank claim. No session suppression:
    /// an unclaim removes a calibration rather than producing a gain.
    pub async fn codex_unclaim(&self, species_name: &str) -> Result<CodexClaimResult, ApiError> {
        let result = self
            .codex
            .unclaim_rank(species_name)
            .await
            .map_err(codex_write_error("codex unclaim"))?;
        Ok(claim_dto(result))
    }

    /// The mastery skill options, ranked by profession contribution or HP
    /// gain exactly as the per-rank recommendation is (several professions
    /// rank by summed weights). Species-independent: the eligible skills
    /// and their fixed rewards are the same for every species whose 25
    /// ranks are complete.
    pub async fn codex_mastery_options(
        &self,
        professions: &[String],
        target: CodexRecommendTarget,
    ) -> Result<Vec<CodexSkillOption>, ApiError> {
        let options = self
            .codex
            .get_mastery_skill_options(professions, target.as_str())
            .await
            .map_err(ApiError::internal("codex mastery options read"))?;
        Ok(options.into_iter().map(skill_option_dto).collect())
    }

    /// Claim a mastery reward for a species whose 25 ranks are complete:
    /// a repeatable claim into any mastery-eligible skill for that skill's
    /// fixed reward. On success, an active session suppresses the claimed
    /// skill's next gain, exactly as a rank claim does.
    pub async fn codex_mastery_claim(
        &self,
        species_name: &str,
        skill_name: &str,
    ) -> Result<CodexMasteryClaimResult, ApiError> {
        let result = self
            .codex
            .mastery_claim(species_name, skill_name)
            .await
            .map_err(codex_write_error("codex mastery claim"))?;
        if self.tracker.is_tracking() {
            self.skill_tracker
                .suppress_next(skill_name, SUPPRESS_TIMEOUT_SECONDS);
        }
        Ok(mastery_claim_dto(result))
    }

    /// Revert a species' most recent mastery claim. No session suppression:
    /// an unclaim removes a calibration rather than producing a gain.
    pub async fn codex_mastery_unclaim(
        &self,
        species_name: &str,
    ) -> Result<CodexMasteryClaimResult, ApiError> {
        let result = self
            .codex
            .mastery_unclaim(species_name)
            .await
            .map_err(codex_write_error("codex mastery unclaim"))?;
        Ok(mastery_claim_dto(result))
    }

    /// Claim a meta codex reward (1 PES into an attribute). On success, an
    /// active session suppresses the attribute's next gain.
    pub async fn codex_meta_claim(
        &self,
        attribute_name: &str,
    ) -> Result<CodexMetaClaimResult, ApiError> {
        let result = self
            .codex
            .meta_claim(attribute_name)
            .await
            .map_err(codex_write_error("codex meta claim"))?;
        if self.tracker.is_tracking() {
            self.skill_tracker
                .suppress_next(attribute_name, SUPPRESS_TIMEOUT_SECONDS);
        }
        Ok(CodexMetaClaimResult {
            attribute_name: result.attribute_name,
            ped_value: result.ped_value,
        })
    }
}

// ── Service-record to DTO mapping ───────────────────────────────────

fn species_dto(entry: codex::SpeciesEntry) -> CodexSpecies {
    CodexSpecies {
        name: entry.name,
        base_cost: entry.base_cost,
        codex_type: entry.codex_type.into(),
        current_rank: entry.current_rank,
        next_rank: entry.next_rank.into(),
        next_category: entry.next_category.map(str::to_string).into(),
        next_cost: entry.next_cost.into(),
        mastery_level: entry.mastery_level,
    }
}

fn species_ranks_dto(ranks: codex::SpeciesRanks) -> CodexSpeciesRanks {
    CodexSpeciesRanks {
        species_name: ranks.species_name,
        base_cost: ranks.base_cost,
        codex_type: ranks.codex_type.into(),
        current_rank: ranks.current_rank,
        mastery_level: ranks.mastery_level,
        mastery_claims: ranks
            .mastery_claims
            .into_iter()
            .map(|claim| CodexMasteryClaim {
                mastery_level: claim.mastery_level,
                skill_name: claim.skill_name,
                ped_value: claim.ped_value,
            })
            .collect(),
        ranks: ranks.ranks.into_iter().map(rank_dto).collect(),
    }
}

fn rank_dto(entry: codex::RankEntry) -> CodexRank {
    CodexRank {
        rank: entry.breakdown.rank,
        category: entry.breakdown.category.to_string(),
        cost: entry.breakdown.cost,
        reward_ped: entry.breakdown.reward_ped,
        cat4_bonus: entry.breakdown.cat4_bonus,
        cat4_reward_ped: entry.breakdown.cat4_reward_ped,
        skills: entry.breakdown.skills,
        cat4_skills: entry.breakdown.cat4_skills,
        claimed: entry.claimed,
        claimed_skill: entry.claimed_skill.into(),
        claimed_ped: entry.claimed_ped.into(),
        is_next: entry.is_next,
    }
}

fn skill_option_dto(option: codex::SkillOption) -> CodexSkillOption {
    CodexSkillOption {
        skill_name: option.skill_name.to_string(),
        category: option.category.to_string(),
        reward_ped: option.reward_ped,
        current_level: option.current_level.into(),
        levels_gained: option.levels_gained,
        profession_weight: option.profession_weight,
        prof_contribution: option.prof_contribution,
        profession_contributions: option
            .profession_contributions
            .into_iter()
            .map(|entry| CodexProfessionContribution {
                profession: entry.profession,
                prof_contribution: entry.prof_contribution,
            })
            .collect(),
        hp_increase: option.hp_increase.into(),
        hp_gain: option.hp_gain,
        recommend_rank: option.recommend_rank.into(),
    }
}

fn claim_dto(record: codex::ClaimRecord) -> CodexClaimResult {
    CodexClaimResult {
        species_name: record.species_name,
        rank: record.rank,
        skill_name: record.skill_name,
        ped_value: record.ped_value,
    }
}

fn mastery_claim_dto(record: codex::MasteryClaimRecord) -> CodexMasteryClaimResult {
    CodexMasteryClaimResult {
        species_name: record.species_name,
        mastery_level: record.mastery_level,
        skill_name: record.skill_name,
        ped_value: record.ped_value,
    }
}

/// The codex writes' error mapping: the service's invalid-input errors
/// carry a user-facing message, so they map to `bad_request`; a driver or rollup failure is an internal error
/// whose detail stays server-side.
fn codex_write_error(context: &'static str) -> impl FnOnce(CodexError) -> ApiError {
    move |err| match err {
        CodexError::Invalid(message) => ApiError::bad_request(message),
        other => ApiError::internal(context)(other),
    }
}

/// Construct the codex service over the facade's shared handles. Kept a
/// free function so [`Api::new`] can build it before the struct is
/// assembled.
pub(crate) fn build_codex_service(
    db: eo_services::db::Db,
    game_data: std::sync::Arc<eo_services::game_data_store::GameDataStore>,
    clock: std::sync::Arc<dyn eo_services::clock::Clock>,
) -> CodexService {
    CodexService::new(db, game_data, clock)
}
