//! The strongbox family: reading an opening from the account site's
//! Inventory Changes export, recording a run of openings, the recorded
//! runs, and taking one back.
//!
//! A run is a conversion in the books (boxes and keys in, what they held
//! out), so recording one moves stock exactly as an item conversion does;
//! the computation lives in [`eo_services::analytics`] and the export
//! reader in [`eo_services::inventory_changes`].

use eo_services::analytics as service;
use eo_services::inventory_changes::{self, UnexpectedReason};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::analytics::{analytics_error, ActivityUndoInput};
use crate::Nullable;
use crate::{Api, ApiError};

// ── Reading an export ───────────────────────────────────────────────

/// An export to read as an opening of one box type.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StrongboxExportInput {
    pub text: String,
    pub box_item: String,
}

/// One item's change in the export.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StrongboxExportRow {
    /// 1-based line number in the pasted text.
    pub line: usize,
    pub item_name: String,
    pub quantity: f64,
    pub value_ped: f64,
}

impl From<inventory_changes::InventoryChange> for StrongboxExportRow {
    fn from(value: inventory_changes::InventoryChange) -> Self {
        Self {
            line: value.line,
            item_name: value.item_name,
            quantity: value.quantity,
            value_ped: value.value_ped,
        }
    }
}

/// Why a row does not fit an opening of the chosen box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StrongboxUnexpectedReason {
    /// Another kind of strongbox moved.
    OtherBox,
    /// Something left that opening a box does not consume.
    Removed,
    /// Boxes or keys arrived rather than left.
    Gained,
    /// The row changed in TT but not in count, as worn gear does.
    Worn,
}

impl From<UnexpectedReason> for StrongboxUnexpectedReason {
    fn from(value: UnexpectedReason) -> Self {
        match value {
            UnexpectedReason::OtherBox => Self::OtherBox,
            UnexpectedReason::Removed => Self::Removed,
            UnexpectedReason::Gained => Self::Gained,
            UnexpectedReason::Worn => Self::Worn,
        }
    }
}

/// A row the opening does not account for.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StrongboxUnexpectedRow {
    pub row: StrongboxExportRow,
    pub reason: StrongboxUnexpectedReason,
}

/// A line of the paste that was not a row.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StrongboxSkippedLine {
    pub line: usize,
    pub text: String,
    pub reason: String,
}

/// The keys the opening used up.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StrongboxKeysUsed {
    pub count: f64,
    pub tt_value: f64,
}

/// An export read as an opening.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StrongboxExportRead {
    pub boxes_opened: f64,
    pub keys: Nullable<StrongboxKeysUsed>,
    /// The Universal Ammo that came back, in PED.
    pub ammo_ped: f64,
    pub contents: Vec<StrongboxExportRow>,
    pub unexpected: Vec<StrongboxUnexpectedRow>,
    pub skipped: Vec<StrongboxSkippedLine>,
    /// TT produced minus the keys' TT: zero for a clean opening.
    pub balance_gap: f64,
}

// ── Recording a run ─────────────────────────────────────────────────

/// Where a run's boxes came from.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StrongboxSource {
    /// Held as stock, looted or otherwise.
    #[serde(rename_all = "camelCase")]
    Held { quantity: f64 },
    /// Held as an asset.
    #[serde(rename_all = "camelCase")]
    Asset { item_id: String, quantity: f64 },
    /// Bought for this run.
    #[serde(rename_all = "camelCase")]
    Bought { quantity: f64, markup_paid: f64 },
}

impl From<StrongboxSource> for service::BoxSource {
    fn from(value: StrongboxSource) -> Self {
        match value {
            StrongboxSource::Held { quantity } => Self::Held { quantity },
            StrongboxSource::Asset { item_id, quantity } => Self::Asset { item_id, quantity },
            StrongboxSource::Bought {
                quantity,
                markup_paid,
            } => Self::Bought {
                quantity,
                markup_paid,
            },
        }
    }
}

/// How a run's result reached the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StrongboxIntake {
    Export,
    Manual,
}

impl From<StrongboxIntake> for service::OpeningIntake {
    fn from(value: StrongboxIntake) -> Self {
        match value {
            StrongboxIntake::Export => Self::Export,
            StrongboxIntake::Manual => Self::Manual,
        }
    }
}

impl From<service::OpeningIntake> for StrongboxIntake {
    fn from(value: service::OpeningIntake) -> Self {
        match value {
            service::OpeningIntake::Export => Self::Export,
            service::OpeningIntake::Manual => Self::Manual,
        }
    }
}

/// One thing the boxes held besides ammo.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StrongboxItemInput {
    pub item_name: String,
    pub quantity: f64,
    pub tt_value: f64,
    /// Turned into Nanocubes rather than kept.
    pub recycle: bool,
}

/// A run of openings to record.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StrongboxOpeningInput {
    pub box_item: String,
    pub sources: Vec<StrongboxSource>,
    pub key_unit_tt: f64,
    pub key_markup_paid: Option<f64>,
    pub contents: Vec<StrongboxItemInput>,
    pub ammo_ped: f64,
    pub intake: StrongboxIntake,
    pub opened_at: Option<String>,
}

/// The run just recorded.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StrongboxOpeningRecorded {
    pub id: String,
}

// ── Recorded runs ───────────────────────────────────────────────────

/// One item of a recorded run, as it stands.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StrongboxOpenedItem {
    pub item_name: String,
    pub quantity: f64,
    pub tt_value: f64,
    /// The Nanocubes it became, while that conversion stands.
    pub nanocubes: Nullable<f64>,
}

/// A recorded run.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StrongboxOpeningEntry {
    pub id: String,
    pub box_item: String,
    pub box_count: f64,
    /// What the boxes cost above TT: bought for the run or held as assets.
    pub box_markup_paid: f64,
    pub key_tt: f64,
    pub key_markup_paid: f64,
    pub ammo_ped: f64,
    pub contents: Vec<StrongboxOpenedItem>,
    pub intake: StrongboxIntake,
    pub opened_at: String,
    pub undone: bool,
}

impl Api {
    /// Read a pasted Inventory Changes export as an opening of one box type.
    /// Pure: nothing is written.
    pub async fn strongbox_export_read(
        &self,
        input: StrongboxExportInput,
    ) -> Result<StrongboxExportRead, ApiError> {
        let read = inventory_changes::read_opening(
            inventory_changes::parse_inventory_changes(&input.text),
            &input.box_item,
        );
        let balance_gap = read.balance_gap();
        Ok(StrongboxExportRead {
            boxes_opened: read.boxes_opened,
            keys: read
                .keys
                .map(|keys| StrongboxKeysUsed {
                    count: keys.count,
                    tt_value: keys.tt_value,
                })
                .into(),
            ammo_ped: read.ammo_ped,
            contents: read.contents.into_iter().map(Into::into).collect(),
            unexpected: read
                .unexpected
                .into_iter()
                .map(|row| StrongboxUnexpectedRow {
                    row: row.change.into(),
                    reason: row.reason.into(),
                })
                .collect(),
            skipped: read
                .skipped
                .into_iter()
                .map(|line| StrongboxSkippedLine {
                    line: line.line,
                    text: line.text,
                    reason: line.reason,
                })
                .collect(),
            balance_gap,
        })
    }

    /// Record a run of openings, with any of what it held turned into
    /// Nanocubes, all or nothing.
    pub async fn strongbox_open(
        &self,
        input: StrongboxOpeningInput,
    ) -> Result<StrongboxOpeningRecorded, ApiError> {
        let id = self
            .analytics
            .open_strongboxes(service::StrongboxOpening {
                box_item: input.box_item,
                sources: input.sources.into_iter().map(Into::into).collect(),
                key_unit_tt: input.key_unit_tt,
                key_markup_paid: input.key_markup_paid.unwrap_or(0.0),
                contents: input
                    .contents
                    .into_iter()
                    .map(|item| service::OpenedItem {
                        item_name: item.item_name,
                        quantity: item.quantity,
                        tt_value: item.tt_value,
                        recycle: item.recycle,
                    })
                    .collect(),
                ammo_ped: input.ammo_ped,
                intake: input.intake.into(),
                opened_at: input.opened_at,
            })
            .await
            .map_err(analytics_error("strongbox opening"))?;
        Ok(StrongboxOpeningRecorded { id })
    }

    /// Every recorded run, newest first.
    pub async fn strongbox_openings(&self) -> Result<Vec<StrongboxOpeningEntry>, ApiError> {
        let rows = self
            .analytics
            .strongbox_openings()
            .await
            .map_err(analytics_error("strongbox openings"))?;
        Ok(rows
            .into_iter()
            .map(|row| StrongboxOpeningEntry {
                id: row.id,
                box_item: row.box_item,
                box_count: row.box_count,
                box_markup_paid: row.box_markup_paid,
                key_tt: row.key_tt,
                key_markup_paid: row.key_markup_paid,
                ammo_ped: row.ammo_ped,
                contents: row
                    .contents
                    .into_iter()
                    .map(|item| StrongboxOpenedItem {
                        item_name: item.item_name,
                        quantity: item.quantity,
                        tt_value: item.tt_value,
                        nanocubes: item.nanocubes.into(),
                    })
                    .collect(),
                intake: row.intake.into(),
                opened_at: row.opened_at,
                undone: row.undone,
            })
            .collect())
    }

    /// Take a run back whole. Refused, with nothing changed, when anything
    /// it produced has since been sold or converted.
    pub async fn strongbox_opening_undo(&self, input: ActivityUndoInput) -> Result<(), ApiError> {
        let existed = self
            .analytics
            .undo_strongbox_opening(&input.id)
            .await
            .map_err(analytics_error("strongbox opening undo"))?;
        if existed {
            Ok(())
        } else {
            Err(ApiError::not_found(
                "no standing opening with that id to undo",
            ))
        }
    }
}
