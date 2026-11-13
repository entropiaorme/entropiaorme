//! Creature detection across the boundary: the running session's mob
//! source as the overlay shows and steers it (the creature pinned, read,
//! carried on from the latest kills, or typed as a backstop; pin and
//! release), and the review after a session: every creature and suggestion
//! the screen gave the session's kills, grouped by creature and grade, and
//! the one action that settles a group (name the creature, or put back the
//! stamp each kill was recorded with).
//!
//! The grade and the pin's kind stay closed vocabularies across the
//! boundary, and a group is named back by the same typed reference the
//! review lists it under. The review service owns every check and repair;
//! this facade maps its outcomes and answers a settlement with the
//! refreshed review.

use eo_services::mob_review::{
    AddedBy, DetectedCreature as ServiceCreature, DetectionGroup, DetectionGroupRef, EntryCheck,
    KeptEntry, MobReviewError, ReviewTarget, SettleOutcome, Settled,
};
use eo_services::tracker::{
    DeclaredMob, MobDetectionGrade as ServiceGrade, MobSourceView as ServiceSourceView,
    PinnedBy as ServicePinnedBy,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tracking::tracker_conflict;
use crate::{Api, ApiError, Nullable};

/// How detection named a creature. The upper four are the screen's own
/// evidence and stamped the kills; the latest-read two are suggestions the
/// kills did not take; the continuity two are the creature being hunted,
/// given where the screen could not settle the kills, and stamped them; the
/// pinned one is an unlisted creature the player pinned from a reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MobDetectionGrade {
    /// A listed creature, read whole and repeatedly in its own fight.
    Named,
    /// A listed creature the screen leaned to without proving it.
    Guess,
    /// A creature on no list, by the spelling read most often.
    Provisional,
    /// A creature on no list, leaned to but not proved.
    ProvisionalGuess,
    /// The listed creature read last before the kill: a suggestion only.
    LatestRead,
    /// The unlisted creature read last before the kill: a suggestion only.
    ProvisionalLatestRead,
    /// The listed creature the latest kills were.
    Continuity,
    /// The unlisted creature the latest kills were.
    ProvisionalContinuity,
    /// An unlisted creature the player pinned from what the screen read:
    /// its spelling still to be confirmed.
    ProvisionalPinned,
}

impl From<ServiceGrade> for MobDetectionGrade {
    fn from(grade: ServiceGrade) -> Self {
        match grade {
            ServiceGrade::Named => Self::Named,
            ServiceGrade::Guess => Self::Guess,
            ServiceGrade::Provisional => Self::Provisional,
            ServiceGrade::ProvisionalGuess => Self::ProvisionalGuess,
            ServiceGrade::LatestRead => Self::LatestRead,
            ServiceGrade::ProvisionalLatestRead => Self::ProvisionalLatestRead,
            ServiceGrade::Continuity => Self::Continuity,
            ServiceGrade::ProvisionalContinuity => Self::ProvisionalContinuity,
            ServiceGrade::ProvisionalPinned => Self::ProvisionalPinned,
        }
    }
}

impl From<MobDetectionGrade> for ServiceGrade {
    fn from(grade: MobDetectionGrade) -> Self {
        match grade {
            MobDetectionGrade::Named => Self::Named,
            MobDetectionGrade::Guess => Self::Guess,
            MobDetectionGrade::Provisional => Self::Provisional,
            MobDetectionGrade::ProvisionalGuess => Self::ProvisionalGuess,
            MobDetectionGrade::LatestRead => Self::LatestRead,
            MobDetectionGrade::ProvisionalLatestRead => Self::ProvisionalLatestRead,
            MobDetectionGrade::Continuity => Self::Continuity,
            MobDetectionGrade::ProvisionalContinuity => Self::ProvisionalContinuity,
            MobDetectionGrade::ProvisionalPinned => Self::ProvisionalPinned,
        }
    }
}

/// A creature as a kill's stamp carries it: its display name, species and
/// maturity (both empty for a creature on no list).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DetectedCreature {
    pub name: String,
    pub species: String,
    pub maturity: String,
}

impl From<ServiceCreature> for DetectedCreature {
    fn from(creature: ServiceCreature) -> Self {
        Self {
            name: creature.name,
            species: creature.species,
            maturity: creature.maturity,
        }
    }
}

impl From<DetectedCreature> for ServiceCreature {
    fn from(creature: DetectedCreature) -> Self {
        Self {
            name: creature.name,
            species: creature.species,
            maturity: creature.maturity,
        }
    }
}

/// Which group of a session's detections: the creature, at one grade,
/// with the session's own key for a creature on no list (two such
/// creatures can share a spelling, read at different levels).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobDetectionGroupRef {
    pub grade: MobDetectionGrade,
    pub creature: DetectedCreature,
    pub provisional_key: Nullable<String>,
}

/// One group of the session's detections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobDetectionGroup {
    pub group: MobDetectionGroupRef,
    pub kill_count: i64,
    /// The text read off the plate most often for the group's kills; null
    /// for the creature being hunted or a pin, which no reading of these
    /// kills gave.
    pub example_text: Nullable<String>,
    /// What the group's kills are recorded as now, when they agree.
    pub current_name: Nullable<String>,
    /// The player has settled the group.
    pub reviewed: bool,
}

/// Every creature and suggestion detection gave an ended session's kills,
/// the least certain first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobDetectionReview {
    pub session_id: String,
    /// The session has ended, so its detections can be settled.
    pub ended: bool,
    pub groups: Vec<MobDetectionGroup>,
}

/// What to settle a group as.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MobDetectionTarget {
    /// The creature named so: a listed creature as the list names it (its
    /// species and maturity, or its species alone), a name confirmed
    /// before, or a new name to keep.
    Creature { name: String },
    /// The stamp each kill was recorded with.
    Declared,
}

/// What a group was settled as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MobDetectionSettled {
    /// A creature on the bundled list (a misspelling of one is taken as it).
    Listed { creature: DetectedCreature },
    /// A name confirmed before.
    Confirmed { creature: DetectedCreature },
    /// A new name, kept from now on as a known creature.
    NewName { creature: DetectedCreature },
    /// Each kill's declared mob.
    Declared,
}

/// What settling a group came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MobDetectionSettleResult {
    /// The group was settled; the session's review as it now stands.
    Settled {
        settled: MobDetectionSettled,
        review: MobDetectionReview,
    },
    /// The name typed came only loosely near this creature: nothing was
    /// changed, and the player may choose it in one action.
    DidYouMean { creature: DetectedCreature },
}

fn review_error(error: MobReviewError) -> ApiError {
    match error {
        MobReviewError::Invalid(message) => ApiError::bad_request(message),
        MobReviewError::NotFound(message) => ApiError::not_found(message),
        MobReviewError::Conflict(message) => ApiError::conflict(message),
        MobReviewError::Db(error) => ApiError::internal("mob detection review")(error),
        MobReviewError::Corrupt(detail) => ApiError::internal("mob detection review")(detail),
    }
}

fn group_view(group: DetectionGroup) -> MobDetectionGroup {
    MobDetectionGroup {
        group: MobDetectionGroupRef {
            grade: group.group.grade.into(),
            creature: group.group.creature.into(),
            provisional_key: group.group.provisional_key.into(),
        },
        kill_count: group.kill_count,
        example_text: group.example_text.into(),
        current_name: group.current_name.into(),
        reviewed: group.reviewed,
    }
}

impl Api {
    /// The creatures and suggestions detection gave a session's kills.
    pub async fn tracking_mob_detections(
        &self,
        session_id: String,
    ) -> Result<MobDetectionReview, ApiError> {
        let review = self
            .mob_review
            .detections(&session_id)
            .await
            .map_err(review_error)?;
        Ok(MobDetectionReview {
            session_id,
            ended: review.ended,
            groups: review.groups.into_iter().map(group_view).collect(),
        })
    }

    /// Settle every kill of one group of an ended session's detections.
    pub async fn tracking_mob_detection_settle(
        &self,
        session_id: String,
        group: MobDetectionGroupRef,
        target: MobDetectionTarget,
    ) -> Result<MobDetectionSettleResult, ApiError> {
        let group = DetectionGroupRef {
            grade: group.grade.into(),
            creature: group.creature.into(),
            provisional_key: group.provisional_key.0,
        };
        let target = match target {
            MobDetectionTarget::Creature { name } => ReviewTarget::Creature(name),
            MobDetectionTarget::Declared => ReviewTarget::Declared,
        };
        let outcome = self
            .mob_review
            .settle(&session_id, group, target)
            .await
            .map_err(review_error)?;
        let settled = match outcome {
            SettleOutcome::DidYouMean(creature) => {
                return Ok(MobDetectionSettleResult::DidYouMean {
                    creature: creature.into(),
                })
            }
            SettleOutcome::Settled(settled) => settled,
        };
        let settled = match settled {
            Settled::Listed(creature) => MobDetectionSettled::Listed {
                creature: creature.into(),
            },
            Settled::Confirmed(creature) => MobDetectionSettled::Confirmed {
                creature: creature.into(),
            },
            Settled::NewName(creature) => MobDetectionSettled::NewName {
                creature: creature.into(),
            },
            Settled::Declared => MobDetectionSettled::Declared,
        };
        Ok(MobDetectionSettleResult::Settled {
            settled,
            review: self.tracking_mob_detections(session_id).await?,
        })
    }
}

/// Where a kept creature came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CreatureEntrySource {
    /// Confirmed in a session's review: retired once no kill carries it.
    Review,
    /// Entered or edited in the creature list: kept until removed.
    Player,
}

/// A creature the player keeps for the name reader beside the bundled list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatureEntry {
    pub name: String,
    pub species: Nullable<String>,
    /// The maturity as entered; a number stands for every number.
    pub maturity: Nullable<String>,
    /// The maturity is a number: read at whatever number the plate prints.
    pub any_number: bool,
    pub source: CreatureEntrySource,
}

impl From<KeptEntry> for CreatureEntry {
    fn from(entry: KeptEntry) -> Self {
        Self {
            name: entry.name,
            species: entry.species.into(),
            maturity: entry.maturity.into(),
            any_number: entry.any_number,
            source: match entry.added_by {
                AddedBy::Review => CreatureEntrySource::Review,
                AddedBy::Player => CreatureEntrySource::Player,
            },
        }
    }
}

/// What keeping a typed creature would come to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CreatureEntryCheck {
    /// Ready to keep as this name, at this maturity when one was typed.
    #[serde(rename_all = "camelCase")]
    Ready {
        name: String,
        maturity: Nullable<String>,
        any_number: bool,
    },
    /// The reader already knows it: by its own name (with the maturity
    /// read, when it has one), and as its kills are recorded.
    #[serde(rename_all = "camelCase")]
    Known {
        name: String,
        maturity: Nullable<String>,
        recorded_as: DetectedCreature,
    },
    /// It cannot be kept, and why.
    Refused { reason: String },
}

impl Api {
    /// The creatures the player keeps for the name reader, oldest first.
    pub async fn creature_entries(&self) -> Result<Vec<CreatureEntry>, ApiError> {
        let entries = self.mob_review.entries().await.map_err(review_error)?;
        Ok(entries.into_iter().map(Into::into).collect())
    }

    /// What keeping `text` (as its plate prints it) would come to, in place
    /// of the kept creature `replacing` when one is being edited.
    pub async fn creature_entry_check(
        &self,
        text: String,
        replacing: Option<String>,
    ) -> Result<CreatureEntryCheck, ApiError> {
        let check = self
            .mob_review
            .check_entry(&text, replacing.as_deref())
            .await
            .map_err(review_error)?;
        Ok(match check {
            EntryCheck::Ready {
                name,
                maturity,
                any_number,
            } => CreatureEntryCheck::Ready {
                name,
                maturity: maturity.into(),
                any_number,
            },
            EntryCheck::Known {
                name,
                maturity,
                creature,
            } => CreatureEntryCheck::Known {
                name,
                maturity: maturity.into(),
                recorded_as: creature.into(),
            },
            EntryCheck::Refused { reason } => CreatureEntryCheck::Refused { reason },
        })
    }

    /// Keep `text` (as its plate prints it), with `species` when given, in
    /// place of the kept creature `replacing`; the list as it now stands.
    pub async fn creature_entry_save(
        &self,
        text: String,
        species: Option<String>,
        replacing: Option<String>,
    ) -> Result<Vec<CreatureEntry>, ApiError> {
        let entries = self
            .mob_review
            .save_entry(&text, species.as_deref(), replacing.as_deref())
            .await
            .map_err(review_error)?;
        Ok(entries.into_iter().map(Into::into).collect())
    }

    /// Stop keeping the creature `name`; the list as it now stands.
    pub async fn creature_entry_remove(
        &self,
        name: String,
    ) -> Result<Vec<CreatureEntry>, ApiError> {
        let entries = self
            .mob_review
            .remove_entry(&name)
            .await
            .map_err(review_error)?;
        Ok(entries.into_iter().map(Into::into).collect())
    }
}

/// How the player pinned a creature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MobPinnedBy {
    /// The creature the screen was reading.
    Read,
    /// A creature the player typed.
    Typed,
}

impl From<ServicePinnedBy> for MobPinnedBy {
    fn from(by: ServicePinnedBy) -> Self {
        match by {
            ServicePinnedBy::Read => Self::Read,
            ServicePinnedBy::Typed => Self::Typed,
        }
    }
}

/// The creature every kill takes while it stands.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobPin {
    pub creature: DetectedCreature,
    pub by: MobPinnedBy,
}

/// The session's one source for its kills' creature, as the overlay shows
/// it. Idle, only the typed creature (the one the next session starts
/// with) is present.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobSourceView {
    /// The screen is being read for the running session. Without it there
    /// is nothing read and nothing carried on from the latest kills, and
    /// the typed creature names every kill.
    pub reading_screen: bool,
    pub pin: Nullable<MobPin>,
    /// The creature the player typed: while the screen is read, the
    /// backstop for a kill nothing else names.
    pub typed: Nullable<DetectedCreature>,
    /// The creature the screen reads now.
    pub reading: Nullable<DetectedCreature>,
    /// The creature the latest kills were, when what is read now does not
    /// name another.
    pub continuity: Nullable<DetectedCreature>,
}

fn creature_view(mob: DeclaredMob) -> DetectedCreature {
    DetectedCreature {
        name: mob.name,
        species: mob.species,
        maturity: mob.maturity,
    }
}

impl From<ServiceSourceView> for MobSourceView {
    fn from(view: ServiceSourceView) -> Self {
        Self {
            reading_screen: view.reading_screen,
            pin: view
                .pin
                .map(|pin| MobPin {
                    creature: creature_view(pin.mob),
                    by: pin.by.into(),
                })
                .into(),
            typed: view.typed.map(creature_view).into(),
            reading: view.reading.map(creature_view).into(),
            continuity: view.continuity.map(creature_view).into(),
        }
    }
}

impl MobSourceView {
    /// The view while no session runs: the creature the next session
    /// starts with, if one is typed.
    pub fn idle(typed: Option<DeclaredMob>) -> Self {
        Self {
            reading_screen: false,
            pin: None.into(),
            typed: typed.map(creature_view).into(),
            reading: None.into(),
            continuity: None.into(),
        }
    }
}

/// What to pin.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MobPinTarget {
    /// The creature the overlay showed as read now (or, with nothing read,
    /// as the one the latest kills were): pinned only if the screen still
    /// shows it.
    Shown { creature: DetectedCreature },
    /// A creature on the list, typed by its species and maturity.
    Typed {
        species: String,
        maturity: Nullable<String>,
    },
}

impl Api {
    /// Pin a creature for the running session: every kill takes it, and the
    /// screen is not read, until it is released. Answers the pin.
    pub async fn tracking_mob_pin(&self, target: MobPinTarget) -> Result<MobPin, ApiError> {
        let pinned = match target {
            MobPinTarget::Shown { creature } => {
                let expected = DeclaredMob {
                    name: creature.name,
                    species: creature.species,
                    maturity: creature.maturity,
                };
                let mob = self
                    .tracker
                    .pin_shown(expected)
                    .await
                    .map_err(tracker_conflict)?;
                MobPin {
                    creature: creature_view(mob),
                    by: MobPinnedBy::Read,
                }
            }
            MobPinTarget::Typed { species, maturity } => {
                let species = species.trim().to_string();
                let maturity = maturity.0.unwrap_or_default().trim().to_string();
                if !eo_services::mob_lookup_service::MobLookupService::new(&self.game_data)
                    .has_mob_name(&species, &maturity)
                {
                    return Err(ApiError::bad_request("Mob is not present in the catalogue"));
                }
                let mob = DeclaredMob::from_parts(species, maturity);
                self.tracker
                    .pin_typed(mob.clone())
                    .await
                    .map_err(tracker_conflict)?;
                MobPin {
                    creature: creature_view(mob),
                    by: MobPinnedBy::Typed,
                }
            }
        };
        Ok(pinned)
    }

    /// Release the running session's pin: the screen is read again when
    /// detection is on. Answers whether a pin stood.
    pub async fn tracking_mob_unpin(&self) -> Result<bool, ApiError> {
        self.tracker.unpin().await.map_err(tracker_conflict)
    }
}
