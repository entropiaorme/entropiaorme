//! Creature detection across the boundary: the running session's mob
//! source as the overlay shows and steers it (the creature pinned, read,
//! carried on from the latest kills, or typed as a backstop; pin and
//! release), and the review after a session: a line for each mob the screen
//! gave the session's kills and one for the kills it could not name, and
//! the one action that settles a line (name the mob, or put back the stamp
//! each kill was recorded with); and the labels the player gives mobs.
//!
//! The pin's kind stays a closed vocabulary across the boundary, and a
//! line is named back by the same typed reference the review lists it
//! under. The review service owns every check and repair;
//! this facade maps its outcomes and answers a settlement with the
//! refreshed review.

use eo_services::mob_review::{
    AddedBy, DetectedCreature as ServiceCreature, EntryCheck, KeptEntry, KeptLabel, MobReviewError,
    ReviewTarget, ReviewedMob as ServiceReviewedMob, ReviewedRef, SettleOutcome, Settled,
};
use eo_services::tracker::{
    DeclaredMob, MobSourceView as ServiceSourceView, PinnedBy as ServicePinnedBy,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tracking::tracker_conflict;
use crate::{Api, ApiError, Nullable};

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

/// Which line of a session's review: a mob detection named, as its kills
/// were recorded (its name and maturity; with the session's own key for a
/// mob on no list, since two such mobs can share a spelling), or the kills
/// the screen could not name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MobReviewRef {
    #[serde(rename_all = "camelCase")]
    Mob {
        species: String,
        maturity: String,
        provisional_key: Nullable<String>,
    },
    Unnamed,
}

impl From<ReviewedRef> for MobReviewRef {
    fn from(mob: ReviewedRef) -> Self {
        match mob {
            ReviewedRef::Mob {
                species,
                maturity,
                provisional_key,
            } => Self::Mob {
                species,
                maturity,
                provisional_key: provisional_key.into(),
            },
            ReviewedRef::Unnamed => Self::Unnamed,
        }
    }
}

impl From<MobReviewRef> for ReviewedRef {
    fn from(mob: MobReviewRef) -> Self {
        match mob {
            MobReviewRef::Mob {
                species,
                maturity,
                provisional_key,
            } => Self::Mob {
                species,
                maturity,
                provisional_key: provisional_key.0,
            },
            MobReviewRef::Unnamed => Self::Unnamed,
        }
    }
}

/// One line of the session's review: a mob and every kill given it, or the
/// kills the screen could not name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewedMob {
    pub mob: MobReviewRef,
    /// The mob as shown; null for the kills the screen could not name.
    pub name: Nullable<String>,
    pub kill_count: i64,
    /// How many of the kills the screen was less than sure of.
    pub unsure_count: i64,
    /// The text read off the plate most often for the mob's kills, when
    /// any was read.
    pub example_text: Nullable<String>,
    /// What the kills are recorded as now, when they agree.
    pub current_name: Nullable<String>,
    /// The mob is on no list: its spelling is the screen's, to be confirmed.
    pub unlisted: bool,
    /// The player has settled the line.
    pub reviewed: bool,
}

/// An ended session's kills by the mob detection gave them: the lines that
/// ask for the player first, the kills the screen could not name last.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobDetectionReview {
    pub session_id: String,
    /// The session has ended, so its kills can be settled.
    pub ended: bool,
    pub mobs: Vec<ReviewedMob>,
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

fn mob_view(mob: ServiceReviewedMob) -> ReviewedMob {
    ReviewedMob {
        mob: mob.mob.into(),
        name: mob.name.into(),
        kill_count: mob.kill_count,
        unsure_count: mob.unsure_count,
        example_text: mob.example_text.into(),
        current_name: mob.current_name.into(),
        unlisted: mob.unlisted,
        reviewed: mob.reviewed,
    }
}

impl Api {
    /// A session's kills by the mob detection gave them.
    pub async fn tracking_mob_detections(
        &self,
        session_id: String,
    ) -> Result<MobDetectionReview, ApiError> {
        let review = self
            .mob_review
            .mobs(&session_id)
            .await
            .map_err(review_error)?;
        Ok(MobDetectionReview {
            session_id,
            ended: review.ended,
            mobs: review.mobs.into_iter().map(mob_view).collect(),
        })
    }

    /// Settle every kill of one line of an ended session's review.
    pub async fn tracking_mob_detection_settle(
        &self,
        session_id: String,
        mob: MobReviewRef,
        target: MobDetectionTarget,
    ) -> Result<MobDetectionSettleResult, ApiError> {
        let target = match target {
            MobDetectionTarget::Creature { name } => ReviewTarget::Creature(name),
            MobDetectionTarget::Declared => ReviewTarget::Declared,
        };
        let outcome = self
            .mob_review
            .settle_mob(&session_id, mob.into(), target)
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

/// A label a mob is shown by.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MobLabel {
    /// The mob as its kills are recorded.
    pub species: String,
    pub maturity: String,
    /// The mob as the game prints it, without any label.
    pub printed: String,
    pub label: String,
    /// The player chose it; otherwise it is how the mob's plate was read
    /// to print it.
    pub by_player: bool,
}

impl From<KeptLabel> for MobLabel {
    fn from(label: KeptLabel) -> Self {
        Self {
            species: label.species,
            maturity: label.maturity,
            printed: label.printed,
            label: label.label,
            by_player: label.by_player,
        }
    }
}

impl Api {
    /// Every label a mob is shown by.
    pub async fn mob_labels(&self) -> Result<Vec<MobLabel>, ApiError> {
        let labels = self.mob_review.labels().await.map_err(review_error)?;
        Ok(labels.into_iter().map(Into::into).collect())
    }

    /// Show `mob` (typed as the game prints it, or as it is labelled now)
    /// as `label`, from now on and everywhere it is recorded; the labels as
    /// they now stand. Which mob it is does not change.
    pub async fn mob_label_set(
        &self,
        mob: String,
        label: String,
    ) -> Result<Vec<MobLabel>, ApiError> {
        let labels = self
            .mob_review
            .set_label(&mob, &label)
            .await
            .map_err(review_error)?;
        Ok(labels.into_iter().map(Into::into).collect())
    }

    /// Show the mob as the game prints it again; the labels as they now
    /// stand.
    pub async fn mob_label_clear(
        &self,
        species: String,
        maturity: String,
    ) -> Result<Vec<MobLabel>, ApiError> {
        let labels = self
            .mob_review
            .clear_label(&species, &maturity)
            .await
            .map_err(review_error)?;
        Ok(labels.into_iter().map(Into::into).collect())
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
