//! What the account readings propose, and the readings a strongbox run
//! asks for.
//!
//! A proposal is a guess: a strongbox opening, a listing sold, a listing
//! returned, a purchase, or a sale to another player, seen between two
//! readings. Nothing here records one.
//! The frontend reads a proposal and hands the player's confirmed result to
//! the existing strongbox or listing command as a normal input; or the
//! player dismisses it. Its certainty crosses the wire typed: whether an
//! opening settled and why not, and whether the PED Card's rise in its
//! step can be read.
//!
//! Recording an opening from proposals is three steps, so an interruption
//! at any point shows instead of passing silently: claim the proposals (in
//! one transaction), record the opening through the strongbox command, then
//! link the proposals to it as one. A recording refused or failed releases
//! the proposals this attempt claimed. A claim with no link (the app closed
//! between the two, or the link failed) keeps the proposals listed as
//! possibly recorded already; a player who has checked takes such a claim
//! over in the next claim (`retake`), so the warning is never lost on the
//! way. A listing's outcome is recorded by the listing's own command, and
//! its proposal linked after.
//!
//! A strongbox run is read on request: when it begins, every minute while
//! it is open, and once more when it is done. Each run command answers
//! once its reading is stored and examined, so the proposals it led to can
//! be read at once, with a typed outcome when it could not be read. Like
//! every account command, these are the main window's alone.

use eo_services::account::composite::Settlement;
use eo_services::account::proposals::{Decided, Detail, Doubt, Proposal, ProposalId, ProposalKind};
use eo_services::account::recorder::RunReading;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::account::{
    account_error, amount, card, iso, opened, place, span, AccountPlace, MeasuredAmount,
    MeasuredBoxes, MeasuredEventItem, MeasuredSettlement, MeasuredSpan,
};
use crate::Nullable;
use crate::{Api, ApiError};

/// What a proposal is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AccountProposalKind {
    Opening,
    ListingSold,
    ListingReturned,
    Purchase,
    /// Something that arrived during a tracked session that no loot line,
    /// recorded reward, conversion or opening explains.
    Arrival,
    /// One item that left for more than its TT outside every session.
    Sale,
}

impl From<AccountProposalKind> for ProposalKind {
    fn from(value: AccountProposalKind) -> Self {
        match value {
            AccountProposalKind::Opening => ProposalKind::Opening,
            AccountProposalKind::ListingSold => ProposalKind::ListingSold,
            AccountProposalKind::ListingReturned => ProposalKind::ListingReturned,
            AccountProposalKind::Purchase => ProposalKind::Purchase,
            AccountProposalKind::Arrival => ProposalKind::Arrival,
            AccountProposalKind::Sale => ProposalKind::Sale,
        }
    }
}

/// Why an opening did not settle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AccountOpeningDoubt {
    /// More than one kind of box left together.
    SeveralKinds,
    /// Boxes left with no keys beside them.
    NoKeys,
    /// The keys and the boxes left in different counts.
    CountsDiffer,
    /// No one reading of the ammo and what arrived pays the keys back.
    Unbalanced,
}

/// The ammo an opening paid back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountProposedAmmo {
    pub units: i64,
    pub amount: MeasuredAmount,
}

/// What a proposal says happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AccountProposalOutcome {
    /// A strongbox opening.
    #[serde(rename_all = "camelCase")]
    Opening {
        /// The boxes that left, one entry for each kind.
        boxes: Vec<MeasuredBoxes>,
        keys_used: i64,
        /// The keys' TT.
        keys: MeasuredAmount,
        /// The ammo it paid back; `null` unless it settled.
        ammo: Nullable<AccountProposedAmmo>,
        /// What every reading of the step takes as its contents.
        contents: Vec<MeasuredEventItem>,
        /// What arrived that may be its contents, and may not.
        possible_contents: Vec<MeasuredEventItem>,
        settlement: MeasuredSettlement,
        /// Why it did not settle; `null` when it did.
        doubt: Nullable<AccountOpeningDoubt>,
        /// How many boxes paid ammo alone; `null` unless the ammo arrived
        /// one row a box and the rows show it exactly.
        paid_ammo_alone: Nullable<i64>,
    },
    /// A listing left the auction with no return: it sold.
    #[serde(rename_all = "camelCase")]
    ListingSold {
        listing_id: String,
        /// The item, as the account site names it.
        item_name: String,
        units: i64,
        /// The TT the readings showed leaving.
        tt: MeasuredAmount,
        /// The PED Card's rise in the same step, offered as the price only
        /// when nothing else could have moved the Card and the readings
        /// were at most three minutes apart; `null` otherwise.
        proceeds: Nullable<MeasuredAmount>,
    },
    /// A listing moved out of the auction into another place: it expired
    /// or was withdrawn.
    #[serde(rename_all = "camelCase")]
    ListingReturned {
        listing_id: String,
        item_name: String,
        units: i64,
        tt: MeasuredAmount,
        returned_to: AccountPlace,
    },
    /// One item arrived as the PED Card fell by more than its TT, outside
    /// every tracked session: bought at auction, its markup paid. Recorded
    /// by the player as an asset purchase, and linked to that purchase,
    /// never recorded on its own.
    #[serde(rename_all = "camelCase")]
    Purchase {
        /// The item, as the account site names it.
        item_name: String,
        units: i64,
        /// Its TT, as the readings show it.
        tt: MeasuredAmount,
        /// The PED Card's fall.
        paid: MeasuredAmount,
        /// What it cost above its TT.
        markup: MeasuredAmount,
        /// Where it arrived.
        arrived_in: AccountPlace,
    },
    /// Something arrived during a tracked session that no loot line,
    /// recorded quest reward, Shrapnel conversion or strongbox opening
    /// explains: a purchase, a trade, a gift or a pickup. The player records
    /// it as an asset purchase (claimed, the purchase recorded, then linked
    /// to the purchase's id), as stock of unknown origin, or dismisses it.
    #[serde(rename_all = "camelCase")]
    Arrival {
        /// The item, as the account site names it.
        item_name: String,
        units: i64,
        /// Its TT, as the readings show it.
        tt: MeasuredAmount,
        /// The PED Card fell by its TT in the same step: bought at the
        /// Trade Terminal, at no markup.
        bought_at_tt: bool,
        /// Where it arrived.
        arrived_in: AccountPlace,
    },
    /// One item left as the PED Card rose by more than its TT, outside
    /// every tracked session: sold to another player, at the Card's rise.
    /// Recorded by the player as a trade (of loot or of an asset), and
    /// linked to that trade, never recorded on its own.
    #[serde(rename_all = "camelCase")]
    Sale {
        /// The item, as the account site names it.
        item_name: String,
        units: i64,
        /// Its TT, as the readings show it.
        tt: MeasuredAmount,
        /// The PED Card's rise.
        received: MeasuredAmount,
        /// What it fetched above its TT.
        markup: MeasuredAmount,
        /// Where it left from.
        left_from: AccountPlace,
    },
}

/// A pending proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountProposal {
    pub id: i64,
    /// The reading it was seen in, and the reading before it.
    pub reading_id: i64,
    pub previous_reading_id: i64,
    /// When those two readings were taken: it happened in between.
    pub between: MeasuredSpan,
    /// The tracked session it fell in; `null` outside one.
    pub session_id: Nullable<String>,
    /// Claimed for a recording that was never linked (the app closed
    /// between the two, or the link failed): it may be recorded already.
    pub possibly_recorded: bool,
    /// When the readings found it: new to the player while later than
    /// what they have seen of the account's activity.
    pub detected_at: String,
    pub outcome: AccountProposalOutcome,
}

/// Proposals to claim before the opening they show is recorded, or to
/// release when it was not.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountProposalsInput {
    pub ids: Vec<i64>,
}

/// Proposals to claim before the opening they show is recorded. `retake`
/// names those among them whose earlier claim, never linked, the player
/// has checked and takes over; every other must not be claimed already.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountProposalsClaimInput {
    pub ids: Vec<i64>,
    #[serde(default)]
    pub retake: Vec<i64>,
}

/// Proposals linked, as one, to the record they became: the strongbox
/// opening's id, the listing's, or for a purchase the asset purchase's id
/// (the highest purchase id on the asset `inventory_create` or
/// `inventory_purchase_add` returns).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountProposalsLinkInput {
    pub ids: Vec<i64>,
    pub record_id: String,
}

/// A proposal the player says is not what happened.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountProposalDismissInput {
    pub id: i64,
}

/// What a reading a strongbox run asked for came to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum AccountRunReading {
    /// Taken, stored and examined: the proposals it led to can be read.
    #[serde(rename_all = "camelCase")]
    Read { reading_id: i64, taken_at: String },
    /// The account is not connected.
    NotConnected,
    /// The account site no longer accepts the session: sign in again.
    SignedOut,
    /// The account site could not be read.
    Unreadable,
    /// The run was closed, or the app is closing, before the reading came.
    Interrupted,
}

/// A strongbox run to read whole, from the reading before its boxes to
/// its latest.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountRunWholeInput {
    pub from_reading_id: i64,
    pub to_reading_id: i64,
    /// The box the run opens, as the account names it.
    pub box_item: String,
}

/// What reading a run whole came to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountRunWhole {
    /// An opening read whole across the run stands among the proposals,
    /// in place of the steps it spans.
    pub read_whole: bool,
    /// How many of the run's box the latest reading still holds: none left
    /// means no more can have been opened.
    pub boxes_left: i64,
}

fn doubt(value: Doubt) -> AccountOpeningDoubt {
    match value {
        Doubt::SeveralKinds => AccountOpeningDoubt::SeveralKinds,
        Doubt::NoKeys => AccountOpeningDoubt::NoKeys,
        Doubt::CountsDiffer => AccountOpeningDoubt::CountsDiffer,
        Doubt::Unbalanced => AccountOpeningDoubt::Unbalanced,
    }
}

fn proposal(value: &Proposal) -> Option<AccountProposal> {
    let outcome = match (&value.detail, value.kind) {
        (Detail::Opening(opening), ProposalKind::Opening) => AccountProposalOutcome::Opening {
            boxes: opening
                .boxes
                .iter()
                .map(|boxes| MeasuredBoxes {
                    name: boxes.name.clone(),
                    count: boxes.count,
                })
                .collect(),
            keys_used: opening.keys_used,
            keys: amount(opening.keys),
            ammo: opening
                .ammo
                .map(|ammo| AccountProposedAmmo {
                    units: ammo.units,
                    amount: amount(ammo.amount),
                })
                .into(),
            contents: opening.contents.iter().map(opened).collect(),
            possible_contents: opening.possible_contents.iter().map(opened).collect(),
            settlement: match opening.settlement {
                Settlement::Settled => MeasuredSettlement::Settled,
                Settlement::Unsettled => MeasuredSettlement::Unsettled,
            },
            doubt: opening.doubt.map(doubt).into(),
            paid_ammo_alone: opening.paid_ammo_alone.into(),
        },
        (Detail::Listing(listing), ProposalKind::ListingSold) => {
            AccountProposalOutcome::ListingSold {
                listing_id: listing.listing_id.clone(),
                item_name: listing.name.clone(),
                units: listing.units,
                tt: amount(listing.amount),
                proceeds: listing.proceeds.map(card).into(),
            }
        }
        (Detail::Listing(listing), ProposalKind::ListingReturned) => {
            AccountProposalOutcome::ListingReturned {
                listing_id: listing.listing_id.clone(),
                item_name: listing.name.clone(),
                units: listing.units,
                tt: amount(listing.amount),
                returned_to: place(listing.returned_to.as_ref()?),
            }
        }
        (Detail::Purchase(bought), ProposalKind::Purchase) => AccountProposalOutcome::Purchase {
            item_name: bought.key.name.clone(),
            units: bought.units,
            tt: amount(bought.amount),
            paid: card(bought.paid),
            markup: amount(bought.markup),
            arrived_in: place(&bought.key.container),
        },
        (Detail::Arrival(arrival), ProposalKind::Arrival) => AccountProposalOutcome::Arrival {
            item_name: arrival.key.name.clone(),
            units: arrival.units,
            tt: amount(arrival.amount),
            bought_at_tt: arrival.bought_at_tt,
            arrived_in: place(&arrival.key.container),
        },
        (Detail::Sale(sold), ProposalKind::Sale) => AccountProposalOutcome::Sale {
            item_name: sold.key.name.clone(),
            units: sold.units,
            tt: amount(sold.amount),
            received: card(sold.received),
            markup: amount(sold.markup),
            left_from: place(&sold.key.container),
        },
        _ => return None,
    };
    Some(AccountProposal {
        id: value.id.0,
        reading_id: value.reading.0,
        previous_reading_id: value.prev_reading.0,
        between: span(&value.between),
        session_id: value.session_id.clone().into(),
        possibly_recorded: value.claimed,
        detected_at: iso(value.detected_at),
        outcome,
    })
}

fn decided(context: &'static str, value: Decided) -> Result<(), ApiError> {
    match value {
        Decided::Done => Ok(()),
        Decided::NotFound => Err(ApiError::not_found(format!(
            "no such proposal to {context}"
        ))),
        Decided::Refused(reason) => Err(ApiError::conflict(reason)),
    }
}

fn ids(values: &[i64]) -> Vec<ProposalId> {
    values.iter().copied().map(ProposalId).collect()
}

fn run_reading(value: RunReading) -> AccountRunReading {
    match value {
        RunReading::Read { reading, taken_at } => AccountRunReading::Read {
            reading_id: reading.0,
            taken_at: iso(taken_at),
        },
        RunReading::NotConnected => AccountRunReading::NotConnected,
        RunReading::SignedOut => AccountRunReading::SignedOut,
        RunReading::Unreadable => AccountRunReading::Unreadable,
        RunReading::Interrupted => AccountRunReading::Interrupted,
    }
}

impl Api {
    /// Every pending proposal, of one kind or all, the latest seen first.
    /// A read: nothing is written.
    pub async fn account_proposals(
        &self,
        kind: Option<AccountProposalKind>,
    ) -> Result<Vec<AccountProposal>, ApiError> {
        let found = self
            .account()?
            .proposals(kind.map(Into::into))
            .await
            .map_err(account_error("account proposals"))?;
        Ok(found.iter().filter_map(proposal).collect())
    }

    /// Claim proposals, all or none and in one transaction, before the
    /// opening they show is recorded through the strongbox command: each
    /// must be open, and not claimed already unless it is to be retaken.
    pub async fn account_proposals_claim(
        &self,
        input: AccountProposalsClaimInput,
    ) -> Result<(), ApiError> {
        let outcome = self
            .account()?
            .claim_proposals(ids(&input.ids), ids(&input.retake))
            .await
            .map_err(account_error("account proposals claim"))?;
        decided("claim", outcome)
    }

    /// Release proposals claimed for a recording the strongbox command
    /// refused or that failed.
    pub async fn account_proposals_release(
        &self,
        input: AccountProposalsInput,
    ) -> Result<(), ApiError> {
        let outcome = self
            .account()?
            .release_proposals(ids(&input.ids))
            .await
            .map_err(account_error("account proposals release"))?;
        decided("release", outcome)
    }

    /// Link proposals, as one, to the record they became, after the
    /// existing strongbox or listing command recorded it. The record must
    /// stand and be the outcome each proposed; linking again to the same
    /// record changes nothing.
    pub async fn account_proposals_link(
        &self,
        input: AccountProposalsLinkInput,
    ) -> Result<(), ApiError> {
        let record = input.record_id.trim().to_string();
        if record.is_empty() {
            return Err(ApiError::bad_request("proposals are linked to a record"));
        }
        let outcome = self
            .account()?
            .link_proposals(ids(&input.ids), record)
            .await
            .map_err(account_error("account proposals link"))?;
        decided("link", outcome)
    }

    /// Dismiss a proposal: the player says it is not what happened.
    pub async fn account_proposal_dismiss(
        &self,
        input: AccountProposalDismissInput,
    ) -> Result<(), ApiError> {
        let outcome = self
            .account()?
            .dismiss_proposal(ProposalId(input.id))
            .await
            .map_err(account_error("account proposal dismiss"))?;
        decided("dismiss", outcome)
    }

    /// A strongbox run begins: a reading from before the boxes are opened,
    /// then one every minute while it is open.
    pub async fn account_run_begin(&self) -> Result<AccountRunReading, ApiError> {
        Ok(run_reading(self.account()?.run_begin().await))
    }

    /// A strongbox run is done: one more reading as soon as the minute
    /// floor allows, then reading for it stops.
    pub async fn account_run_done(&self) -> Result<AccountRunReading, ApiError> {
        Ok(run_reading(self.account()?.run_done().await))
    }

    /// Read a strongbox run whole, from the reading before its boxes to
    /// its latest: every arrival a content, the ammo as the Universal Ammo
    /// gained. Stored as one opening proposal across the run when it
    /// balances; the run's proposals are then read as before. A later
    /// reading read whole replaces the result rather than adding to it.
    pub async fn account_run_whole(
        &self,
        input: AccountRunWholeInput,
    ) -> Result<AccountRunWhole, ApiError> {
        if input.to_reading_id < input.from_reading_id {
            return Err(ApiError::bad_request("A run ends after it begins"));
        }
        let read = self
            .account()?
            .run_whole(
                eo_services::account::ReadingId(input.from_reading_id),
                eo_services::account::ReadingId(input.to_reading_id),
                input.box_item,
            )
            .await
            .map_err(account_error("account run whole"))?;
        Ok(AccountRunWhole {
            read_whole: read.read_whole,
            boxes_left: read.boxes_left,
        })
    }

    /// Close a strongbox run without another reading: its view went away.
    pub async fn account_run_cancel(&self) -> Result<(), ApiError> {
        self.account()?.run_cancel();
        Ok(())
    }
}
