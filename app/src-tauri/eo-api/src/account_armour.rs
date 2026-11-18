//! The limited armour and plates the player holds, as the Armour tab shows
//! them: each piece with the markup it is billed at, and the items of the
//! latest reading that may be declared a piece.
//!
//! A piece's wear is read from the account readings and billed at the
//! markup set here as it stood when the session started (the first one set
//! covers the piece's wear from the start), or at the TT it lost when none
//! was. Declaring an item a piece, or taking it back off the list, is the
//! cost placement every unplaced item takes (`account_cost_place` with a
//! limited category, `account_cost_placement_clear`): one record, so the
//! Armour tab and the placement list never disagree. Like every account
//! command, these are the main window's alone.

use eo_services::account::armour::{ArmourPieces, CandidateHint, PieceList};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::account::{account_error, iso};
use crate::Nullable;
use crate::{Api, ApiError};

/// The highest markup a piece may be set to, in percent.
pub const MAX_PIECE_MARKUP_PERCENT: f64 = 100_000.0;

/// Which list a piece is shown in: for the player's order only, since both
/// bill alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ArmourPieceList {
    Armour,
    Plates,
}

/// One limited piece.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArmourPieceRow {
    /// The item's exact name, as the account names it.
    pub item: String,
    pub list: ArmourPieceList,
    /// Whether the latest reading holds it.
    pub held: bool,
    /// Its TT in PED in the latest reading; `null` when not held.
    pub tt_ped: Nullable<f64>,
    /// The markup it is billed at from now on, in percent; `null` bills
    /// it at the TT it loses.
    pub markup_percent: Nullable<f64>,
    /// When that markup was set; `null` when none was.
    pub markup_set_at: Nullable<String>,
    /// Whether the player declared it a piece (it can be taken back off
    /// the list); otherwise its own name makes it one.
    pub declared: bool,
    /// When a reading showed its TT rise after its markup was set: a new
    /// piece of the same name, whose markup may differ.
    pub replaced_at: Nullable<String>,
}

/// Why an item is offered first when declaring a piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ArmourCandidateHint {
    /// It wore in a measured session and nothing places it yet.
    AwaitingPlacement,
    /// It sits attached to another item, as a plate does.
    Attached,
    /// Its name carries the gender tag armour carries.
    Tagged,
    Other,
}

/// An item of the latest reading that may be declared a piece.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArmourCandidateRow {
    pub item: String,
    pub hint: ArmourCandidateHint,
}

/// The Armour tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArmourPiecesView {
    /// Held first, then by list and name.
    pub pieces: Vec<ArmourPieceRow>,
    /// The likeliest first, then by name.
    pub candidates: Vec<ArmourCandidateRow>,
    /// When the latest reading was taken; `null` before any.
    pub read_at: Nullable<String>,
}

/// A piece's markup, set from now on.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArmourPieceMarkupInput {
    pub item: String,
    /// In percent, from 100; `null` bills the piece at TT from now on.
    pub markup_percent: Option<f64>,
}

fn view(pieces: &ArmourPieces) -> ArmourPiecesView {
    ArmourPiecesView {
        pieces: pieces
            .pieces
            .iter()
            .map(|piece| ArmourPieceRow {
                item: piece.item.clone(),
                list: match piece.list {
                    PieceList::Armour => ArmourPieceList::Armour,
                    PieceList::Plates => ArmourPieceList::Plates,
                },
                held: piece.held,
                tt_ped: piece.tt.map(|tt| tt.0 as f64 / 100.0).into(),
                markup_percent: piece.markup_percent.into(),
                markup_set_at: piece.markup_set_at.map(iso).into(),
                declared: piece.declared,
                replaced_at: piece.replaced_at.map(iso).into(),
            })
            .collect(),
        candidates: pieces
            .candidates
            .iter()
            .map(|candidate| ArmourCandidateRow {
                item: candidate.item.clone(),
                hint: match candidate.hint {
                    CandidateHint::AwaitingPlacement => ArmourCandidateHint::AwaitingPlacement,
                    CandidateHint::Attached => ArmourCandidateHint::Attached,
                    CandidateHint::Tagged => ArmourCandidateHint::Tagged,
                    CandidateHint::Other => ArmourCandidateHint::Other,
                },
            })
            .collect(),
        read_at: pieces.read_at.map(iso).into(),
    }
}

/// Whether a markup may be set: from 100% (a limited piece is never bought
/// below its TT) to [`MAX_PIECE_MARKUP_PERCENT`].
fn valid_markup(markup: Option<f64>) -> Result<Option<f64>, ApiError> {
    match markup {
        None => Ok(None),
        Some(percent)
            if percent.is_finite() && (100.0..=MAX_PIECE_MARKUP_PERCENT).contains(&percent) =>
        {
            Ok(Some((percent * 100.0).round() / 100.0))
        }
        Some(_) => Err(ApiError::bad_request(
            "A limited piece's markup is from 100% up",
        )),
    }
}

impl Api {
    /// The limited armour and plates the player holds, each with the
    /// markup it is billed at, and the items that may be declared one.
    pub async fn account_armour_pieces(&self) -> Result<ArmourPiecesView, ApiError> {
        let pieces = self
            .account()?
            .armour_pieces()
            .await
            .map_err(account_error("account armour pieces"))?;
        Ok(view(&pieces))
    }

    /// Set the markup a limited piece is billed at from now on (the first
    /// one set covers its wear from the start), rebuild the measured costs
    /// it moves, and answer with the tab as it now stands.
    pub async fn account_armour_piece_markup(
        &self,
        input: ArmourPieceMarkupInput,
    ) -> Result<ArmourPiecesView, ApiError> {
        let item = input.item.trim().to_string();
        if item.is_empty() {
            return Err(ApiError::bad_request("An item name is required"));
        }
        let markup = valid_markup(input.markup_percent)?;
        let account = self.account()?;
        account
            .set_piece_markup(item, markup)
            .await
            .map_err(account_error("account armour piece markup"))?;
        self.account_armour_pieces().await
    }
}

#[cfg(test)]
mod tests {
    use super::valid_markup;

    #[test]
    fn a_markup_is_from_one_hundred_percent_and_rounded_to_the_hundredth() {
        assert_eq!(valid_markup(None).unwrap(), None);
        assert_eq!(valid_markup(Some(120.654)).unwrap(), Some(120.65));
        assert_eq!(valid_markup(Some(100.0)).unwrap(), Some(100.0));
        assert!(valid_markup(Some(99.99)).is_err());
        assert!(valid_markup(Some(f64::NAN)).is_err());
        assert!(valid_markup(Some(100_000.01)).is_err());
    }
}
