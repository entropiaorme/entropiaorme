//! Reconciling the app's stock against what the account shows.
//!
//! A reconciliation is planned from the latest account reading, or from an
//! inventory the player pasted from the account site's Copy CSV (which needs
//! no connection), into a draft kept in the backend: per item, the app's
//! quantity beside the account's, and what to do about the difference.
//! Every revision is validated before it is kept, so whatever edits the
//! draft (the Inventory tab, or anything later) shares one write path.
//! Applying it records each line through the existing stock writes, in one
//! transaction against the stock as it stands; undoing it takes back
//! everything it recorded. Like every account command, these are the main
//! window's alone.

use eo_services::account::reconcile::PlanError;
use eo_services::analytics::{
    ApplyOutcome, Caution, Disposition, ExcludedItem, Exclusion, LineChoice, LineRefusal,
    ReconciliationDraft, ReconciliationLine, ReconciliationSource, ReconciliationUndo, Revision,
    UnitTt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::account::{account_error, iso};
use crate::analytics::{analytics_error, ActivityUndoInput};
use crate::Nullable;
use crate::{Api, ApiError};

/// What a reconciliation was observed from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StockReconciliationSource {
    /// The account reading stored last.
    #[serde(rename_all = "camelCase")]
    Reading { reading_id: i64 },
    /// An inventory pasted from the account site.
    Paste,
}

/// Why an item is left out of a reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StockReconciliationExclusion {
    /// Liquid PED, never stock.
    UniversalAmmo,
    /// Ammunition other than Universal Ammo.
    Ammo,
    Equipment,
    Armour,
    Plate,
    /// Rows attached to another item.
    Attached,
    /// Rows listed at auction.
    Auction,
    /// More of it is listed at auction than the app has listings for.
    AuctionWithoutListing,
    /// A proposal waits on the item.
    PendingProposal,
    /// Used up as a cost, and not held as stock.
    Consumable,
    /// Strongboxes and their keys, not held as stock: the opening flow's.
    Strongbox,
    /// One item to a row that the app has never held as stock.
    SingleItem,
    /// Two of the app's items share the name, and both are held.
    SharedName,
    /// The account's TT for it does not fit the item's TT per unit: another
    /// item of the same name, or one whose TT has changed.
    TtDiffers,
    /// More is missing than the loose stock holds: the rest is an asset,
    /// which only its own records move.
    HeldAsAsset,
}

/// An item, or some of its rows, left out, with why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockReconciliationExcluded {
    pub item_name: String,
    /// What the account shows of it under this reason.
    pub quantity: f64,
    pub reason: StockReconciliationExclusion,
}

/// An item's TT per unit, and how far it can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StockReconciliationUnitTt {
    /// The app's own, or a constant of the game.
    Exact { ped: f64 },
    /// From the account's values, truncated to the PEC per row: at most a
    /// PEC a row low.
    Truncated { ped: f64 },
    /// Not known.
    Unknown,
}

/// What to do about one item's difference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StockReconciliationDisposition {
    /// The missing units leave the open positions.
    Removed,
    /// The missing Shrapnel was converted into Universal Ammo, all but
    /// `soldAtTt` units sold at TT (`null` for none).
    #[serde(rename_all = "camelCase")]
    ShrapnelConverted { sold_at_tt: Nullable<f64> },
    /// One item of a conversion: its shortfalls went in, its surplus came
    /// out.
    #[serde(rename_all = "camelCase")]
    ConversionGroup { group: u32 },
    /// The extra units are held as stock of unknown origin.
    UnknownOrigin,
    /// Nothing is recorded for it.
    LeftAsIs,
}

/// One item whose quantity in the app differs from the account's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockReconciliationLine {
    pub item_name: String,
    /// Everything the app holds of it, its assets included.
    pub app_quantity: f64,
    /// The part of `appQuantity` held as assets, which a reconciliation
    /// never moves: a shortfall leaves the loose stock alone.
    pub asset_quantity: f64,
    pub observed_quantity: f64,
    /// The account's quantity less the app's: below zero when units are
    /// missing.
    pub difference: f64,
    pub unit_tt: StockReconciliationUnitTt,
    pub disposition: StockReconciliationDisposition,
    /// A record the account readings made of this item was taken back and
    /// no reconciliation has settled the item since: why it is left as it
    /// is unless the player says otherwise.
    pub taken_back: bool,
    /// When a standing reconciliation accepted this same difference, if
    /// one did.
    pub accepted_at: Nullable<String>,
}

/// One conversion group's figures: the TT that went in and came out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockReconciliationGroup {
    pub group: u32,
    /// The TT of the missing items that went in.
    pub tt_in: f64,
    /// The TT of what came out.
    pub tt_out: f64,
    /// The item that came out, how much of it the conversion produced, and
    /// how much more arrived, held as of unknown origin (a recycling makes
    /// no more Nanocubes than its inputs' TT pays for).
    pub output_item: String,
    pub output_quantity: f64,
    pub remainder_quantity: f64,
}

/// Something to know before applying a draft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StockReconciliationCaution {
    /// A paste planned within the hour after a session ended may have been
    /// copied before it: copy the inventory afresh after a session.
    #[serde(rename_all = "camelCase")]
    PasteSoonAfterSession { session_ended_at: String },
}

/// A reconciliation being reviewed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockReconciliationDraft {
    pub id: String,
    pub source: StockReconciliationSource,
    /// When the account was observed.
    pub observed_at: String,
    /// The largest difference by TT first.
    pub lines: Vec<StockReconciliationLine>,
    pub excluded: Vec<StockReconciliationExcluded>,
    /// Each conversion group's figures.
    pub groups: Vec<StockReconciliationGroup>,
    /// `null` when there is nothing to say.
    pub caution: Nullable<StockReconciliationCaution>,
    /// Lines whose difference a standing reconciliation accepted, standing
    /// as they were: left as they are unless brought back into review.
    pub previously_accepted: Vec<StockReconciliationLine>,
}

/// A line a bulk choice did not fit, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockReconciliationUnchanged {
    pub item_name: String,
    pub reason: String,
}

/// A revised draft, and the lines a bulk choice was not set on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockReconciliationRevised {
    pub draft: StockReconciliationDraft,
    pub unchanged: Vec<StockReconciliationUnchanged>,
}

/// An inventory pasted from the account site's Copy CSV.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockReconciliationPasteInput {
    pub text: String,
}

/// A disposition a line can be set to on its own.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StockReconciliationChoice {
    Removed,
    ShrapnelConverted,
    UnknownOrigin,
    LeftAsIs,
}

/// One change to the draft.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StockReconciliationRevision {
    /// Set the disposition of these items' lines, one or many: a filter
    /// arrives as the items the shared search matched.
    Set {
        items: Vec<String>,
        choice: StockReconciliationChoice,
    },
    /// How much of the missing Shrapnel was sold at TT; `null` for none.
    ShrapnelSoldAtTt { quantity: Option<f64> },
    /// Make these lines one conversion: one surplus and its shortfalls.
    Group { items: Vec<String> },
    /// Bring previously accepted lines back into review.
    Reconsider { items: Vec<String> },
}

/// A revision of the draft `draftId`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockReconciliationReviseInput {
    pub draft_id: String,
    pub revision: StockReconciliationRevision,
}

/// The draft to apply.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockReconciliationApplyInput {
    pub draft_id: String,
}

/// Why a line was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StockReconciliationRefusal {
    /// Loot or a stock movement of the item was recorded after the account
    /// was observed.
    StockMovedSince,
    /// What the app holds of it changed after the plan.
    ChangedSincePlanned,
    /// Another item of its conversion was refused.
    GroupRefused,
}

/// A line left for the next plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StockReconciliationRefused {
    pub item_name: String,
    pub reason: StockReconciliationRefusal,
}

/// What applying a draft came to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum StockReconciliationApplied {
    /// Applied: the reconciliation's id (`null` when nothing was recorded:
    /// every line refused, and no record kept), what was settled or left as
    /// it is, and what was refused.
    #[serde(rename_all = "camelCase")]
    Applied {
        reconciliation_id: Nullable<String>,
        settled: Vec<String>,
        refused: Vec<StockReconciliationRefused>,
    },
    /// Applied before: nothing changed.
    #[serde(rename_all = "camelCase")]
    AlreadyApplied { reconciliation_id: String },
    /// A session is being tracked: nothing changed.
    SessionTracked,
}

fn source(value: &ReconciliationSource) -> StockReconciliationSource {
    match value {
        ReconciliationSource::Reading { reading_id } => StockReconciliationSource::Reading {
            reading_id: *reading_id,
        },
        ReconciliationSource::Paste => StockReconciliationSource::Paste,
    }
}

fn exclusion(value: Exclusion) -> StockReconciliationExclusion {
    match value {
        Exclusion::UniversalAmmo => StockReconciliationExclusion::UniversalAmmo,
        Exclusion::Equipment => StockReconciliationExclusion::Equipment,
        Exclusion::Armour => StockReconciliationExclusion::Armour,
        Exclusion::Plate => StockReconciliationExclusion::Plate,
        Exclusion::Attached => StockReconciliationExclusion::Attached,
        Exclusion::Auction => StockReconciliationExclusion::Auction,
        Exclusion::PendingProposal => StockReconciliationExclusion::PendingProposal,
        Exclusion::Consumable => StockReconciliationExclusion::Consumable,
        Exclusion::Strongbox => StockReconciliationExclusion::Strongbox,
        Exclusion::SingleItem => StockReconciliationExclusion::SingleItem,
        Exclusion::SharedName => StockReconciliationExclusion::SharedName,
        Exclusion::TtDiffers => StockReconciliationExclusion::TtDiffers,
        Exclusion::Ammo => StockReconciliationExclusion::Ammo,
        Exclusion::AuctionWithoutListing => StockReconciliationExclusion::AuctionWithoutListing,
        Exclusion::HeldAsAsset => StockReconciliationExclusion::HeldAsAsset,
    }
}

fn excluded(value: &ExcludedItem) -> StockReconciliationExcluded {
    StockReconciliationExcluded {
        item_name: value.item_name.clone(),
        quantity: value.quantity,
        reason: exclusion(value.reason),
    }
}

fn unit_tt(value: UnitTt) -> StockReconciliationUnitTt {
    match value {
        UnitTt::Exact { ped } => StockReconciliationUnitTt::Exact { ped },
        UnitTt::Truncated { ped } => StockReconciliationUnitTt::Truncated { ped },
        UnitTt::Unknown => StockReconciliationUnitTt::Unknown,
    }
}

fn disposition(value: &Disposition) -> StockReconciliationDisposition {
    match value {
        Disposition::Removed => StockReconciliationDisposition::Removed,
        Disposition::ShrapnelConverted { sold_at_tt } => {
            StockReconciliationDisposition::ShrapnelConverted {
                sold_at_tt: (*sold_at_tt).into(),
            }
        }
        Disposition::ConversionGroup { group } => {
            StockReconciliationDisposition::ConversionGroup { group: *group }
        }
        Disposition::UnknownOrigin => StockReconciliationDisposition::UnknownOrigin,
        Disposition::LeftAsIs => StockReconciliationDisposition::LeftAsIs,
    }
}

fn line(value: &ReconciliationLine) -> StockReconciliationLine {
    StockReconciliationLine {
        item_name: value.item_name.clone(),
        app_quantity: value.app_quantity,
        observed_quantity: value.observed_quantity,
        difference: value.difference(),
        unit_tt: unit_tt(value.unit_tt),
        disposition: disposition(&value.disposition),
        taken_back: !value.taken_back.is_empty(),
        asset_quantity: value.asset_quantity,
        accepted_at: value.accepted_at.map(iso).into(),
    }
}

fn draft(value: &ReconciliationDraft) -> StockReconciliationDraft {
    StockReconciliationDraft {
        id: value.id.clone(),
        source: source(&value.source),
        observed_at: iso(value.observed_at),
        lines: value.lines.iter().map(line).collect(),
        previously_accepted: value.previously_accepted.iter().map(line).collect(),
        excluded: value.excluded.iter().map(excluded).collect(),
        groups: value
            .groups
            .iter()
            .map(|figures| StockReconciliationGroup {
                group: figures.group,
                tt_in: figures.tt_in,
                tt_out: figures.tt_out,
                output_item: figures.output_item.clone(),
                output_quantity: figures.output_quantity,
                remainder_quantity: figures.remainder_quantity,
            })
            .collect(),
        caution: value
            .caution
            .map(|caution| match caution {
                Caution::PasteSoonAfterSession { session_ended_at } => {
                    StockReconciliationCaution::PasteSoonAfterSession {
                        session_ended_at: iso(session_ended_at),
                    }
                }
            })
            .into(),
    }
}

fn revision(value: StockReconciliationRevision) -> Revision {
    match value {
        StockReconciliationRevision::Set { items, choice } => Revision::Set {
            items,
            choice: match choice {
                StockReconciliationChoice::Removed => LineChoice::Removed,
                StockReconciliationChoice::ShrapnelConverted => LineChoice::ShrapnelConverted,
                StockReconciliationChoice::UnknownOrigin => LineChoice::UnknownOrigin,
                StockReconciliationChoice::LeftAsIs => LineChoice::LeftAsIs,
            },
        },
        StockReconciliationRevision::ShrapnelSoldAtTt { quantity } => {
            Revision::ShrapnelSoldAtTt { quantity }
        }
        StockReconciliationRevision::Group { items } => Revision::Group { items },
        StockReconciliationRevision::Reconsider { items } => Revision::Reconsider { items },
    }
}

fn refusal(value: &LineRefusal) -> StockReconciliationRefusal {
    match value {
        LineRefusal::StockMovedSince => StockReconciliationRefusal::StockMovedSince,
        LineRefusal::ChangedSincePlanned => StockReconciliationRefusal::ChangedSincePlanned,
        LineRefusal::GroupRefused => StockReconciliationRefusal::GroupRefused,
    }
}

fn plan_error(context: &'static str) -> impl FnOnce(PlanError) -> ApiError {
    move |error| match error {
        PlanError::NoReading => ApiError::conflict(
            "no account reading has been stored yet: sign in, or paste the inventory",
        ),
        PlanError::Paste(refusal) => ApiError::bad_request(refusal.describe()),
        PlanError::Analytics(error) => analytics_error(context)(error),
        PlanError::Account(error) => account_error(context)(error),
    }
}

impl Api {
    /// Plan a stock reconciliation from the latest account reading, kept as
    /// the draft being reviewed (replacing any other). A conflict when no
    /// reading has been stored; refused while a session is tracked.
    pub async fn account_reconciliation_plan(&self) -> Result<StockReconciliationDraft, ApiError> {
        let planned = self
            .account()?
            .reconciliation_from_reading()
            .await
            .map_err(plan_error("account reconciliation plan"))?;
        Ok(draft(&planned))
    }

    /// Plan a stock reconciliation from an inventory pasted from the account
    /// site's Copy CSV, kept as the draft being reviewed. Needs no
    /// connection. A paste with a row that does not read is refused whole,
    /// naming its line.
    pub async fn account_reconciliation_paste(
        &self,
        input: StockReconciliationPasteInput,
    ) -> Result<StockReconciliationDraft, ApiError> {
        let planned = self
            .account()?
            .reconciliation_from_paste(input.text)
            .await
            .map_err(plan_error("account reconciliation paste"))?;
        Ok(draft(&planned))
    }

    /// The draft being reviewed; `null` when there is none. A read.
    pub async fn account_reconciliation_draft(
        &self,
    ) -> Result<Nullable<StockReconciliationDraft>, ApiError> {
        let found = self
            .analytics
            .reconciliation_draft()
            .await
            .map_err(analytics_error("account reconciliation draft"))?;
        Ok(found.as_ref().map(draft).into())
    }

    /// Revise the draft: validated whole and kept only when it fits; the
    /// reason is a bad request otherwise. A choice set on several lines is
    /// set on those it fits, and the rest come back with why (a bad request
    /// only when it fits none). Refused when another plan has
    /// replaced the draft.
    pub async fn account_reconciliation_revise(
        &self,
        input: StockReconciliationReviseInput,
    ) -> Result<StockReconciliationRevised, ApiError> {
        let revised = self
            .analytics
            .revise_reconciliation(&input.draft_id, revision(input.revision))
            .await
            .map_err(analytics_error("account reconciliation revise"))?;
        Ok(StockReconciliationRevised {
            draft: draft(&revised.draft),
            unchanged: revised
                .unchanged
                .into_iter()
                .map(|(item_name, reason)| StockReconciliationUnchanged { item_name, reason })
                .collect(),
        })
    }

    /// Apply the draft against the stock as it stands, in one transaction.
    /// Applying it again changes nothing. Not found when no such draft is
    /// being reviewed.
    pub async fn account_reconciliation_apply(
        &self,
        input: StockReconciliationApplyInput,
    ) -> Result<StockReconciliationApplied, ApiError> {
        let outcome = self
            .analytics
            .apply_reconciliation(&input.draft_id)
            .await
            .map_err(analytics_error("account reconciliation apply"))?;
        match outcome {
            ApplyOutcome::Applied {
                reconciliation_id,
                settled,
                refused,
            } => Ok(StockReconciliationApplied::Applied {
                reconciliation_id: reconciliation_id.into(),
                settled,
                refused: refused
                    .iter()
                    .map(|(item_name, reason)| StockReconciliationRefused {
                        item_name: item_name.clone(),
                        reason: refusal(reason),
                    })
                    .collect(),
            }),
            ApplyOutcome::AlreadyApplied { reconciliation_id } => {
                Ok(StockReconciliationApplied::AlreadyApplied { reconciliation_id })
            }
            ApplyOutcome::SessionTracked => Ok(StockReconciliationApplied::SessionTracked),
            ApplyOutcome::NoDraft => Err(ApiError::not_found(
                "that reconciliation is no longer being reviewed",
            )),
        }
    }

    /// Undo a reconciliation as a whole, by its id: every record it made is
    /// taken back. Not found when none stands; a conflict, with nothing
    /// changed, when what it produced has since left.
    pub async fn account_reconciliation_undo(
        &self,
        input: ActivityUndoInput,
    ) -> Result<(), ApiError> {
        let undone = self
            .analytics
            .undo_reconciliation(&input.id)
            .await
            .map_err(analytics_error("account reconciliation undo"))?;
        match undone {
            ReconciliationUndo::Undone => Ok(()),
            ReconciliationUndo::NotFound => Err(ApiError::not_found(
                "no reconciliation with that id to take back",
            )),
            ReconciliationUndo::Blocked(reason) => Err(ApiError::conflict(reason)),
        }
    }
}
