//! The review after a session of what the screen read as the tool in hand
//! that is not in the player's equipment library: tools of the game's data
//! held without a library entry, and names that matched no tool. Each is
//! settled once (added to the library, dismissed as not a tracked tool, or
//! left), and a settlement can be undone.
//!
//! What a sighting is of, how it was settled, what a settlement did and
//! why one was refused all cross the boundary as closed vocabularies. The
//! review service owns every check and write; this facade builds a new
//! entry through the equipment add path, tells the held-tool catalogue
//! when the library or the dismissed names change, and answers every
//! settlement with the refreshed review.

use eo_services::config_service::load_config_readonly;
use eo_services::game_ui::tool_names::ToolKind;
use eo_services::tool_review::{
    DismissedName, EntryOnUndo, KeptBecause, NewEntry, Refusal, Settled, SettledKind, Settlement,
    Sighted, ToolReviewError, ToolSighting as ServiceSighting, UndoContext,
};
use eo_wire::normalizer::to_python_json_dumps;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::equipment::{EquipmentKind, EquipmentRequest};
use crate::{Api, ApiError, Nullable};

/// What kind of tool a tool of the game's data is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SightedToolKind {
    Weapon,
    Healing,
    Harvesting,
}

impl From<ToolKind> for SightedToolKind {
    fn from(kind: ToolKind) -> Self {
        match kind {
            ToolKind::Weapon => Self::Weapon,
            ToolKind::Healing => Self::Healing,
            ToolKind::Tool => Self::Harvesting,
        }
    }
}

/// What the screen read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolSighted {
    /// A tool of the game's data the library does not hold, used at its
    /// cost without attachments. `shots` are a weapon's shots booked to it
    /// this session (null while not known).
    Bundled {
        name: String,
        tool: SightedToolKind,
        shots: Nullable<i64>,
    },
    /// A name that matched no tool, as it was read.
    Unknown { text: String },
}

/// How a sighting was settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolSightingSettled {
    /// Added to the library: as a new entry (`created`), or linked to the
    /// entry of that item the library already held.
    Added { created: bool },
    /// Dismissed as not a tracked tool.
    Dismissed,
    /// Left: asked about no more for this session.
    Left,
}

/// A settlement, by what it was alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ToolSightingSettledKind {
    Added,
    Dismissed,
    Left,
}

impl From<SettledKind> for ToolSightingSettledKind {
    fn from(kind: SettledKind) -> Self {
        match kind {
            SettledKind::Added => Self::Added,
            SettledKind::Dismissed => Self::Dismissed,
            SettledKind::Left => Self::Left,
        }
    }
}

/// One thing read off the screen that is not a library entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolSighting {
    pub id: String,
    pub sighted: ToolSighted,
    /// When it was first and last seen in hand (epoch seconds).
    pub first_seen: f64,
    pub last_seen: f64,
    /// Null while it is still to be settled.
    pub settled: Nullable<ToolSightingSettled>,
}

/// A session's sightings: the ones still to be settled first, tools of
/// the game's data before names read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolSightingReview {
    pub session_id: String,
    /// The session has ended, so its tools can be added.
    pub ended: bool,
    pub sightings: Vec<ToolSighting>,
}

/// What to settle a sighting as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ToolSightingAction {
    /// Add the tool to the library (a tool of the game's data only).
    Add,
    /// It is not a tool the player tracks: never ask about the name again.
    Dismiss,
    /// Ask no more about it for this session.
    Leave,
}

/// How an added tool's uses this session are costed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ToolUsesCosting {
    /// A weapon's shots, at its cost without attachments, as they were
    /// booked: what the new entry costs them at too.
    WithoutAttachments,
    /// A harvesting tool's uses, at the tool's cost, as they were booked:
    /// what the new entry costs them at too.
    AsBooked,
    /// Not costed: a healing tool's heals stay unbilled until each is
    /// marked as a paid use in the session's healing review.
    NotCosted,
}

/// Why the entry an addition created stays with the addition undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ToolEntryKeptBecause {
    /// It was set up, renamed or turned off since.
    Changed,
    /// Its tool is no longer in the game's data.
    NotInGameData,
    /// Another detected tool became it.
    OtherSighting,
    /// A session runs now, or ran since it was added.
    InSession,
    /// A correction or a recorded use refers to it.
    Referenced,
    /// It is on a hotbar slot, carried, or a harvesting tool in the
    /// settings.
    InSettings,
}

impl From<KeptBecause> for ToolEntryKeptBecause {
    fn from(because: KeptBecause) -> Self {
        match because {
            KeptBecause::Changed => Self::Changed,
            KeptBecause::NotInGameData => Self::NotInGameData,
            KeptBecause::OtherSighting => Self::OtherSighting,
            KeptBecause::InSession => Self::InSession,
            KeptBecause::Referenced => Self::Referenced,
            KeptBecause::InSettings => Self::InSettings,
        }
    }
}

/// What undoing an addition did with the entry it created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolSightingEntryOnUndo {
    /// No entry this addition created is in the library.
    None,
    /// The entry it created was taken out of the library.
    Removed,
    /// The entry it created stays, for this reason.
    Kept { because: ToolEntryKeptBecause },
}

/// The library entry a tool was added as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolSightingEntry {
    pub id: String,
    /// The entry as the player tells it from others of its name.
    pub label: String,
    /// The entry is on. One linked to may be off, and then the reader
    /// cannot take the tool as in hand until it is turned on.
    pub enabled: bool,
}

/// What a settlement, or its undoing, did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolSightingChange {
    Added {
        created: bool,
        uses: ToolUsesCosting,
        entry: ToolSightingEntry,
    },
    Dismissed,
    Left,
    /// The settlement `was` is undone; the sighting is asked about again.
    Reopened {
        was: ToolSightingSettledKind,
        entry: ToolSightingEntryOnUndo,
    },
}

/// Why a settlement or its undoing was refused. Nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolSightingRefusal {
    /// No such sighting.
    NotFound,
    /// The sighting belongs to another session.
    OtherSession,
    /// It is settled already, as `settled`.
    AlreadySettled { settled: ToolSightingSettledKind },
    /// There is no settlement to undo.
    NotSettled,
    /// A tool is added once its session has ended.
    SessionRunning,
    /// A name that matched no tool cannot be added.
    NotATool,
    /// The tool is no longer in the game's data.
    NotInGameData,
    /// The tool needs choices its data does not give: add it from
    /// Equipment.
    NeedsSetUp,
}

impl From<Refusal> for ToolSightingRefusal {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::NotFound => Self::NotFound,
            Refusal::OtherSession => Self::OtherSession,
            Refusal::AlreadySettled(kind) => Self::AlreadySettled {
                settled: kind.into(),
            },
            Refusal::NotSettled => Self::NotSettled,
            Refusal::SessionRunning => Self::SessionRunning,
            Refusal::NotATool => Self::NotATool,
            Refusal::NotInGameData => Self::NotInGameData,
            Refusal::NeedsSetUp => Self::NeedsSetUp,
        }
    }
}

/// A name set aside as not a tracked tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DismissedToolName {
    pub name: String,
    /// When it was set aside (epoch seconds).
    pub dismissed_at: f64,
}

impl From<DismissedName> for DismissedToolName {
    fn from(name: DismissedName) -> Self {
        Self {
            name: name.name,
            dismissed_at: name.dismissed_at,
        }
    }
}

/// What settling a sighting, or undoing its settlement, came to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolSightingSettleResult {
    /// Done; the session's review as it now stands.
    Settled {
        change: ToolSightingChange,
        review: ToolSightingReview,
    },
    /// Refused: nothing changed.
    Refused { reason: ToolSightingRefusal },
}

fn review_error(error: ToolReviewError) -> ApiError {
    match error {
        ToolReviewError::SessionNotFound => ApiError::not_found("Session not found"),
        ToolReviewError::Db(error) => ApiError::internal("tool sighting review")(error),
        ToolReviewError::Corrupt(detail) => ApiError::internal("tool sighting review")(detail),
    }
}

fn sighting_view(sighting: ServiceSighting) -> ToolSighting {
    ToolSighting {
        id: sighting.id,
        sighted: match sighting.sighted {
            Sighted::Bundled {
                name, kind, shots, ..
            } => ToolSighted::Bundled {
                name,
                tool: kind.into(),
                shots: shots.into(),
            },
            Sighted::Unknown { text } => ToolSighted::Unknown { text },
        },
        first_seen: sighting.first_seen,
        last_seen: sighting.last_seen,
        settled: sighting
            .settled
            .map(|settled| match settled {
                Settled::Added { created } => ToolSightingSettled::Added { created },
                Settled::Dismissed => ToolSightingSettled::Dismissed,
                Settled::Left => ToolSightingSettled::Left,
            })
            .into(),
    }
}

fn change_view(settlement: Settlement, label: Option<String>) -> ToolSightingChange {
    match settlement {
        Settlement::Added {
            created,
            kind,
            equipment_id,
            enabled,
        } => ToolSightingChange::Added {
            created,
            uses: match kind {
                ToolKind::Weapon => ToolUsesCosting::WithoutAttachments,
                ToolKind::Tool => ToolUsesCosting::AsBooked,
                ToolKind::Healing => ToolUsesCosting::NotCosted,
            },
            entry: ToolSightingEntry {
                id: equipment_id.to_string(),
                label: label.unwrap_or_default(),
                enabled,
            },
        },
        Settlement::Dismissed => ToolSightingChange::Dismissed,
        Settlement::Left => ToolSightingChange::Left,
        Settlement::Reopened { was, entry } => ToolSightingChange::Reopened {
            was: was.into(),
            entry: match entry {
                EntryOnUndo::None => ToolSightingEntryOnUndo::None,
                EntryOnUndo::Removed => ToolSightingEntryOnUndo::Removed,
                EntryOnUndo::Kept(because) => ToolSightingEntryOnUndo::Kept {
                    because: because.into(),
                },
            },
        },
    }
}

fn refused(refusal: Refusal) -> ToolSightingSettleResult {
    ToolSightingSettleResult::Refused {
        reason: refusal.into(),
    }
}

impl Api {
    /// What the screen read as the tool in hand during a session that is
    /// not in the library.
    pub async fn tracking_tool_sightings(
        &self,
        session_id: String,
    ) -> Result<ToolSightingReview, ApiError> {
        let review = self
            .tool_review
            .sightings(&session_id)
            .await
            .map_err(review_error)?;
        Ok(ToolSightingReview {
            session_id,
            ended: review.ended,
            sightings: review.sightings.into_iter().map(sighting_view).collect(),
        })
    }

    /// Settle one of a session's sightings.
    pub async fn tracking_tool_sighting_settle(
        &self,
        session_id: String,
        sighting_id: String,
        action: ToolSightingAction,
    ) -> Result<ToolSightingSettleResult, ApiError> {
        let outcome = match action {
            ToolSightingAction::Add => {
                let sighting = match self
                    .tool_review
                    .sighting(&session_id, &sighting_id)
                    .await
                    .map_err(review_error)?
                {
                    Ok(sighting) => sighting,
                    Err(refusal) => return Ok(refused(refusal)),
                };
                if let Some(settled) = sighting.settled {
                    return Ok(refused(Refusal::AlreadySettled(settled.kind())));
                }
                let Sighted::Bundled { kind, item_id, .. } = &sighting.sighted else {
                    return Ok(refused(Refusal::NotATool));
                };
                let entry = match self.bare_entry(*kind, item_id)? {
                    Ok(entry) => entry,
                    Err(refusal) => return Ok(refused(refusal)),
                };
                self.tool_review.add(&session_id, &sighting_id, entry).await
            }
            ToolSightingAction::Dismiss => {
                self.tool_review.dismiss(&session_id, &sighting_id).await
            }
            ToolSightingAction::Leave => self.tool_review.leave(&session_id, &sighting_id).await,
        }
        .map_err(review_error)?;
        self.settled(session_id, outcome).await
    }

    /// Undo a sighting's settlement, so it is asked about again.
    pub async fn tracking_tool_sighting_undo(
        &self,
        session_id: String,
        sighting_id: String,
    ) -> Result<ToolSightingSettleResult, ApiError> {
        let sighting = match self
            .tool_review
            .sighting(&session_id, &sighting_id)
            .await
            .map_err(review_error)?
        {
            Ok(sighting) => sighting,
            Err(refusal) => return Ok(refused(refusal)),
        };
        // What only this side can tell about an entry the addition created:
        // the entry the add path would build now, and whether the settings
        // hold it anywhere.
        let mut context = UndoContext::default();
        if let (Some(Settled::Added { created: true }), Sighted::Bundled { kind, item_id, .. }) =
            (&sighting.settled, &sighting.sighted)
        {
            context.bare = self.bare_entry(*kind, item_id)?.ok();
            context.in_settings = match self.added_entry_id(&sighting.id).await? {
                Some(id) => self.settings_hold(id)?,
                None => false,
            };
        }
        let outcome = self
            .tool_review
            .undo(&session_id, &sighting_id, context)
            .await
            .map_err(review_error)?;
        self.settled(session_id, outcome).await
    }

    /// The names set aside as not tracked tools, oldest first.
    pub async fn equipment_dismissed_tools(&self) -> Result<Vec<DismissedToolName>, ApiError> {
        Ok(self
            .tool_review
            .dismissed()
            .await
            .map_err(review_error)?
            .into_iter()
            .map(DismissedToolName::from)
            .collect())
    }

    /// Bring a name set aside back, so the held-tool reader matches it
    /// again; answers the names still set aside.
    pub async fn equipment_track_again(
        &self,
        name: String,
    ) -> Result<Vec<DismissedToolName>, ApiError> {
        if self
            .tool_review
            .track_again(&name)
            .await
            .map_err(review_error)?
        {
            self.equipment_changed().await;
        }
        self.equipment_dismissed_tools().await
    }

    /// Answer a settlement with the refreshed review, once the held-tool
    /// catalogue follows any change to the library or the dismissed names.
    async fn settled(
        &self,
        session_id: String,
        outcome: Result<Settlement, Refusal>,
    ) -> Result<ToolSightingSettleResult, ApiError> {
        let settlement = match outcome {
            Ok(settlement) => settlement,
            Err(refusal) => return Ok(refused(refusal)),
        };
        let catalogue_changed = matches!(
            settlement,
            Settlement::Added { created: true, .. }
                | Settlement::Dismissed
                | Settlement::Reopened {
                    was: SettledKind::Dismissed,
                    ..
                }
                | Settlement::Reopened {
                    entry: EntryOnUndo::Removed,
                    ..
                }
        );
        if catalogue_changed {
            self.equipment_changed().await;
        }
        let label = match settlement {
            Settlement::Added { equipment_id, .. } => self.entry_label(equipment_id).await?,
            _ => None,
        };
        Ok(ToolSightingSettleResult::Settled {
            change: change_view(settlement, label),
            review: self.tracking_tool_sightings(session_id).await?,
        })
    }

    /// The entry as the player tells it from the others of its name, by
    /// the one rule every surface naming library entries uses.
    async fn entry_label(&self, equipment_id: i64) -> Result<Option<String>, ApiError> {
        let rows = self
            .db
            .equipment_library_rows()
            .await
            .map_err(ApiError::internal("equipment library read"))?;
        let Some(name) = rows
            .iter()
            .find(|row| row.0 == equipment_id)
            .map(|row| row.1.clone())
        else {
            return Ok(None);
        };
        let alike: Vec<&crate::held_tool::LibraryRow> =
            rows.iter().filter(|row| row.1 == name).collect();
        let labels = crate::held_tool::lookalike_labels(&alike);
        Ok(alike
            .iter()
            .zip(labels)
            .find(|(row, _)| row.0 == equipment_id)
            .map(|(_, label)| label))
    }

    /// The entry an addition made the sighting into, while it exists.
    async fn added_entry_id(&self, sighting_id: &str) -> Result<Option<i64>, ApiError> {
        let sighting_id = sighting_id.to_string();
        self.db
            .with_reader(move |conn| {
                use rusqlite::OptionalExtension as _;
                Ok(conn
                    .query_row(
                        "SELECT equipment_id FROM tool_sightings WHERE id = ?1",
                        [sighting_id],
                        |row| row.get::<_, Option<i64>>(0),
                    )
                    .optional()?
                    .flatten())
            })
            .await
            .map_err(ApiError::internal("tool sighting entry read"))
    }

    /// Whether the player's settings hold the entry: bound to a slot on
    /// any hotbar preset, carried, or named as a harvesting tool.
    fn settings_hold(&self, equipment_id: i64) -> Result<bool, ApiError> {
        let config =
            load_config_readonly(&self.data_dir).map_err(ApiError::internal("settings read"))?;
        let guardrail = &config.harvest_guardrail;
        Ok(config.hotbar_ids_live_first().contains(&equipment_id)
            || config.carried_weapon_ids.contains(&equipment_id)
            || [
                guardrail.short_tool_id,
                guardrail.long_tool_id,
                guardrail.huge_tool_id,
            ]
            .contains(&Some(equipment_id)))
    }

    /// The library entry the equipment add path builds for a tool of the
    /// game's data picked with nothing else chosen, or why it cannot be
    /// built: the tool has left the game's data, or it is a healer whose
    /// data gives no heal to build its healing from.
    fn bare_entry(
        &self,
        kind: ToolKind,
        item_id: &str,
    ) -> Result<Result<NewEntry, Refusal>, ApiError> {
        let equipment_kind = match kind {
            ToolKind::Weapon => EquipmentKind::Weapon,
            ToolKind::Healing => EquipmentKind::Healing,
            ToolKind::Tool => EquipmentKind::Tool,
        };
        match self.build_props(&EquipmentRequest::bare(equipment_kind, item_id)) {
            Ok(built) => Ok(Ok(NewEntry {
                name: built.name,
                properties_json: to_python_json_dumps(&built.props),
            })),
            Err(ApiError::NotFound { .. }) => Ok(Err(Refusal::NotInGameData)),
            // The healing profile's checks are the only refusals a bare
            // healer's build can meet.
            Err(ApiError::BadRequest { .. }) if kind == ToolKind::Healing => {
                Ok(Err(Refusal::NeedsSetUp))
            }
            Err(error) => Err(error),
        }
    }
}
