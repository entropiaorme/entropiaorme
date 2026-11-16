//! The account connection family: the connection's state, disconnecting,
//! and what the account readings measure of tracked sessions. What the
//! readings propose, and the readings a strongbox run asks for, are in
//! [`crate::account_proposals`].
//!
//! Signing in happens in a window the shell opens, and the session it
//! yields moves from the shell to the connection in Rust alone: no command
//! here takes or returns it. Measured figures stand beside the tracked
//! ones and never feed them; the list of measured sessions reads the
//! tracked figures from each session's summary as it stands.
//!
//! Every amount crosses the wire in PED with its precision beside it:
//! `exact` when a quantity and an exact unit value reproduce every value
//! it was read from, `wholePec` when it rests on values the site truncated
//! to the whole PEC (each may hide up to one PEC more).

use std::sync::Arc;

use eo_services::account::amount::{Amount, Pec, Precision, MICRO_PER_PED};
use eo_services::account::composite::{OpenedContent, Opening, RepairCost, Settlement};
use eo_services::account::diff::{MoveKind, Span};
use eo_services::account::measure::{
    AmmoSplit, CoverageLevel, Edge, GainMark, ItemUnits, NotableEvent, SessionMeasure,
};
use eo_services::account::reading::{Container, HoldingKey};
use eo_services::account::roles::{Role, Stacking};
use eo_services::account::sessions::{
    self, AmmoTotal, LeftOutReason, MeasureRead, MeasureTotals, MeasuredSession, SummaryPair,
    TrackedFigures,
};
use eo_services::account::{AccountError, AccountRecorder, Confirming, ConnectionState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tracking::SessionExclusion;
use crate::Nullable;
use crate::{Api, ApiError};

/// The connection's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AccountConnectionState {
    Disconnected,
    /// A kept session is being confirmed: the site could not be reached
    /// yet, which is not the same as having been signed out.
    Unconfirmed,
    Connected,
    /// The site refused the session during this run: signing in again
    /// restores the connection. Nothing is read meanwhile.
    Expired,
}

/// The connection as the player reads it at a glance: on the dashboard
/// and on the overlay. One answer for every surface, so they never
/// disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AccountSignal {
    /// The player has not connected, or disconnected: nothing to show
    /// beyond the offer to sign in.
    Off,
    /// The player connected before and is not signed in in this run of the
    /// app: their usual state at launch.
    NotSignedIn,
    /// A kept sign-in is being confirmed with the site.
    Checking,
    SignedIn,
    /// The site ended the sign-in while the app was running.
    SignedOut,
}

impl AccountSignal {
    /// The signal of a connection's state, for a player who has or has not
    /// connected.
    pub fn of(state: AccountConnectionState, has_connected: bool) -> AccountSignal {
        match state {
            AccountConnectionState::Connected => AccountSignal::SignedIn,
            AccountConnectionState::Expired => AccountSignal::SignedOut,
            AccountConnectionState::Unconfirmed => AccountSignal::Checking,
            AccountConnectionState::Disconnected if has_connected => AccountSignal::NotSignedIn,
            AccountConnectionState::Disconnected => AccountSignal::Off,
        }
    }
}

/// The account connection as it stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub state: AccountConnectionState,
    /// The connection at a glance; the same answer [`account_signal`]
    /// gives every window.
    ///
    /// [`account_signal`]: crate::Api::account_signal
    pub signal: AccountSignal,
    /// When the current connection was made.
    pub connected_at: Nullable<String>,
    /// When the latest reading was taken.
    pub last_reading_at: Nullable<String>,
    /// Whether a tracked session is being read now (running, or waiting
    /// for its closing readings).
    pub reading_session: bool,
    /// Whether the player has connected and not disconnected since.
    pub has_connected: bool,
    /// While a kept session is being confirmed (`unconfirmed`), whether
    /// the site could be reached; `null` in every other state.
    pub confirming: Nullable<AccountConfirming>,
    /// Whether the sign-in window is open.
    pub signing_in: bool,
    /// Whether any reading has ever been stored, connected or not: the
    /// measures stay reviewable after a disconnect.
    pub has_readings: bool,
}

/// How confirming a kept session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AccountConfirming {
    /// No attempt has failed to reach the site yet.
    Checking,
    /// The last attempt could not reach the site; it is tried again.
    SiteUnreachable,
}

/// How an amount was derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MeasuredPrecision {
    Exact,
    WholePec,
}

/// An amount in PED, with how exactly it is known.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredAmount {
    pub ped: f64,
    pub precision: MeasuredPrecision,
}

pub(crate) fn amount(value: Amount) -> MeasuredAmount {
    MeasuredAmount {
        ped: value.micro_ped() as f64 / MICRO_PER_PED as f64,
        precision: match value.precision() {
            Precision::Exact => MeasuredPrecision::Exact,
            Precision::WholePec => MeasuredPrecision::WholePec,
        },
    }
}

/// A PED Card figure: whole PEC.
pub(crate) fn card(value: Pec) -> MeasuredAmount {
    MeasuredAmount {
        ped: value.0 as f64 / 100.0,
        precision: MeasuredPrecision::WholePec,
    }
}

/// Where a holding sits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AccountPlace {
    Carried,
    Auction,
    Storage {
        planet: String,
    },
    Attached {
        parent: String,
    },
    /// A place the app does not know, under the site's name for it.
    Other {
        name: String,
    },
}

pub(crate) fn place(container: &Container) -> AccountPlace {
    match container {
        Container::Carried => AccountPlace::Carried,
        Container::Auction => AccountPlace::Auction,
        Container::Storage { planet } => AccountPlace::Storage {
            planet: planet.clone(),
        },
        Container::Attached { parent } => AccountPlace::Attached {
            parent: parent.clone(),
        },
        Container::Other { name } => AccountPlace::Other { name: name.clone() },
    }
}

/// An item in one place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountHolding {
    pub name: String,
    pub place: AccountPlace,
}

pub(crate) fn holding(key: &HoldingKey) -> AccountHolding {
    AccountHolding {
        name: key.name.clone(),
        place: place(&key.container),
    }
}

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MeasuredRole {
    Ammo,
    Weapon,
    Amplifier,
    Healing,
    Armour,
    Plating,
    OtherEquipment,
    Material,
}

fn role(value: Role) -> MeasuredRole {
    match value {
        Role::Ammo => MeasuredRole::Ammo,
        Role::Weapon => MeasuredRole::Weapon,
        Role::Amplifier => MeasuredRole::Amplifier,
        Role::Healing => MeasuredRole::Healing,
        Role::Armour => MeasuredRole::Armour,
        Role::Plating => MeasuredRole::Plating,
        Role::OtherEquipment => MeasuredRole::OtherEquipment,
        Role::Material => MeasuredRole::Material,
    }
}

/// Whether a measure's readings bracket the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MeasuredCoverageLevel {
    /// Read close to both ends, with no gap between readings.
    Complete,
    Partial,
}

fn coverage_level(level: CoverageLevel) -> MeasuredCoverageLevel {
    match level {
        CoverageLevel::Complete => MeasuredCoverageLevel::Complete,
        CoverageLevel::Partial => MeasuredCoverageLevel::Partial,
    }
}

/// Whether one end of the session was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MeasuredEdge {
    Read,
    Unread,
}

fn edge(value: Edge) -> MeasuredEdge {
    match value {
        Edge::Read => MeasuredEdge::Read,
        Edge::Unread => MeasuredEdge::Unread,
    }
}

/// How well the readings cover the session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredCoverage {
    pub level: MeasuredCoverageLevel,
    pub opening: MeasuredEdge,
    pub closing: MeasuredEdge,
    pub reading_count: i64,
    /// Intervals between consecutive readings longer than 150 seconds.
    pub gap_count: i64,
}

/// When something happened: after one reading, by the next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredSpan {
    pub from: String,
    pub to: String,
}

fn wire_state(state: ConnectionState) -> AccountConnectionState {
    match state {
        ConnectionState::Disconnected => AccountConnectionState::Disconnected,
        ConnectionState::Unconfirmed => AccountConnectionState::Unconfirmed,
        ConnectionState::Connected => AccountConnectionState::Connected,
        ConnectionState::Expired => AccountConnectionState::Expired,
    }
}

pub(crate) fn iso(at: f64) -> String {
    match eo_services::tracking_reads::list_ts_to_iso(Some(at)) {
        serde_json::Value::String(text) => text,
        other => other.to_string(),
    }
}

pub(crate) fn span(value: &Span) -> MeasuredSpan {
    MeasuredSpan {
        from: iso(value.from),
        to: iso(value.to),
    }
}

/// How an ammo type's change splits into burned and paid back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MeasuredAmmoSplit {
    /// Every strongbox opening settled: what was burned is the change less
    /// what openings and converted Shrapnel paid back.
    #[serde(rename_all = "camelCase")]
    Settled {
        burned_units: i64,
        burned: MeasuredAmount,
        strongbox_units: i64,
        strongbox: MeasuredAmount,
        converted_units: i64,
        converted: MeasuredAmount,
    },
    /// An opening did not settle, so how much it paid back is not known;
    /// the units its rows suggest are given only as that reading of them.
    #[serde(rename_all = "camelCase")]
    Unsettled {
        burned_units_by_rows: i64,
        strongbox_units_by_rows: i64,
        converted_units: i64,
    },
}

/// The ammo of one type a session used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredAmmo {
    pub name: String,
    /// The change in units, always exact.
    pub net_units: i64,
    pub net: MeasuredAmount,
    pub split: MeasuredAmmoSplit,
}

/// One item's wear across the session, never netted against repairs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredDecay {
    pub item: AccountHolding,
    pub role: MeasuredRole,
    pub decay: MeasuredAmount,
}

/// One item's share of a figure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredItemAmount {
    pub item: AccountHolding,
    pub amount: MeasuredAmount,
}

/// What a repair cost, as far as the PED Card shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MeasuredRepairCost {
    /// The Card's fall, with nothing else in its minute moving it.
    Paid { amount: MeasuredAmount },
    /// What remains of the Card's change beside a sale or a purchase in
    /// the same minute: within what the restorations allow, but resting on
    /// those figures too.
    Derived { amount: MeasuredAmount },
    /// The Card's movement cannot be read as the repair's cost.
    Unknown,
}

/// A repair: what it restored, and what the PED Card shows it cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredRepair {
    pub between: MeasuredSpan,
    pub items: Vec<MeasuredItemAmount>,
    pub restored: MeasuredAmount,
    pub cost: MeasuredRepairCost,
}

/// A stackable consumed that is not ammo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredConsumption {
    pub item: AccountHolding,
    pub role: MeasuredRole,
    pub units: i64,
    pub amount: MeasuredAmount,
}

/// Whether an item's rows count units or hold one item each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MeasuredStacking {
    Stack,
    Single,
}

/// Whether something gained may be a strongbox's contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MeasuredGainMark {
    Arrived,
    PossiblyStrongbox,
    /// Arrived in a minute whose PED Card movement nothing explains: it
    /// may have been bought.
    CardMovedUnexplained,
}

/// What arrived of one holding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredGain {
    pub item: AccountHolding,
    pub role: MeasuredRole,
    pub stacking: MeasuredStacking,
    pub mark: MeasuredGainMark,
    pub units: i64,
    pub amount: MeasuredAmount,
}

/// One line of an opening, a sale or a purchase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredEventItem {
    pub item: AccountHolding,
    pub units: i64,
    pub amount: MeasuredAmount,
}

/// Boxes of one kind an opening used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredBoxes {
    pub name: String,
    pub count: i64,
}

/// Whether an opening's ammo was read on one reading of its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MeasuredSettlement {
    Settled,
    Unsettled,
}

/// What a move between places is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MeasuredMoveKind {
    Listing,
    ListingReturned,
    Transfer,
}

/// Something worth listing that is not a cost or a gain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MeasuredEvent {
    #[serde(rename_all = "camelCase")]
    StrongboxOpening {
        between: MeasuredSpan,
        /// The boxes that left, one entry for each kind.
        boxes: Vec<MeasuredBoxes>,
        keys_used: i64,
        /// The keys' TT: the deposit the boxes pay back.
        keys: MeasuredAmount,
        ammo_units: i64,
        ammo_rows: i64,
        ammo_from_rows: MeasuredAmount,
        ammo_from_keys: MeasuredAmount,
        /// What every way of closing the keys' balance takes.
        contents: Vec<MeasuredEventItem>,
        /// What some ways take: possibly the opening's.
        possible_contents: Vec<MeasuredEventItem>,
        settlement: MeasuredSettlement,
    },
    #[serde(rename_all = "camelCase")]
    Moved {
        between: MeasuredSpan,
        name: String,
        units: i64,
        amount: MeasuredAmount,
        from: AccountPlace,
        to: AccountPlace,
        move_kind: MeasuredMoveKind,
    },
    /// A single item arrived (it also counts in what was gained).
    #[serde(rename_all = "camelCase")]
    Appeared {
        between: MeasuredSpan,
        item: AccountHolding,
        units: i64,
        value: MeasuredAmount,
    },
    /// A single item went; whether its TT was spent is not known.
    #[serde(rename_all = "camelCase")]
    Left {
        between: MeasuredSpan,
        item: AccountHolding,
        units: i64,
        value: MeasuredAmount,
    },
    /// What left, sold to the Trade Terminal, as the one reading of the
    /// Card's change in its minute has it.
    #[serde(rename_all = "camelCase")]
    Sale {
        between: MeasuredSpan,
        items: Vec<MeasuredEventItem>,
        amount: MeasuredAmount,
    },
    /// What arrived, bought at TT, as the one reading of the Card's change
    /// in its minute has it.
    #[serde(rename_all = "camelCase")]
    Purchase {
        between: MeasuredSpan,
        items: Vec<MeasuredEventItem>,
        amount: MeasuredAmount,
    },
    #[serde(rename_all = "camelCase")]
    ShrapnelConverted {
        between: MeasuredSpan,
        shrapnel_units: i64,
        shrapnel: MeasuredAmount,
        ammo_units: i64,
        ammo: MeasuredAmount,
    },
    #[serde(rename_all = "camelCase")]
    ListingSold {
        between: MeasuredSpan,
        item: AccountHolding,
        units: i64,
        amount: MeasuredAmount,
    },
    /// The PED Card moved with nothing in the step known to explain it.
    #[serde(rename_all = "camelCase")]
    CardChanged {
        between: MeasuredSpan,
        from: MeasuredAmount,
        to: MeasuredAmount,
    },
}

/// A tracked session's measured account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountSessionMeasure {
    pub session_id: String,
    /// Final: the session has ended and its closing readings are in.
    pub sealed: bool,
    pub coverage: MeasuredCoverage,
    pub ammo: Vec<MeasuredAmmo>,
    pub decay: Vec<MeasuredDecay>,
    pub repairs: Vec<MeasuredRepair>,
    pub other_consumption: Vec<MeasuredConsumption>,
    /// Stacks that left with nothing to explain them: never a cost.
    pub unexplained_departures: Vec<MeasuredConsumption>,
    /// What arrived that no opening claimed, no purchase paid for and no
    /// move brought: the figure tracked loot compares with.
    pub gained: Vec<MeasuredGain>,
    pub gained_total: MeasuredAmount,
    /// Of the gained total, what may be strongbox contents.
    pub gained_possibly_strongbox: MeasuredAmount,
    pub events: Vec<MeasuredEvent>,
    /// The measure by category, as the measured sessions list carries it.
    pub totals: MeasuredTotals,
}

fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn coverage(measure: &SessionMeasure) -> MeasuredCoverage {
    let coverage = measure.coverage;
    MeasuredCoverage {
        level: coverage_level(coverage.level),
        opening: edge(coverage.opening),
        closing: edge(coverage.closing),
        reading_count: count(coverage.reading_count),
        gap_count: count(coverage.gap_count),
    }
}

pub(crate) fn opened(content: &OpenedContent) -> MeasuredEventItem {
    MeasuredEventItem {
        item: holding(&content.key),
        units: content.units,
        amount: amount(content.amount),
    }
}

fn opening(between: &Span, opening: &Opening) -> MeasuredEvent {
    MeasuredEvent::StrongboxOpening {
        between: span(between),
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
        ammo_units: opening.ammo_units,
        ammo_rows: count(opening.ammo_rows),
        ammo_from_rows: amount(opening.ammo_from_rows),
        ammo_from_keys: amount(opening.ammo_from_keys),
        contents: opening.contents.iter().map(opened).collect(),
        possible_contents: opening.possible_contents.iter().map(opened).collect(),
        settlement: match opening.settlement {
            Settlement::Settled => MeasuredSettlement::Settled,
            Settlement::Unsettled => MeasuredSettlement::Unsettled,
        },
    }
}

fn event(value: &NotableEvent) -> MeasuredEvent {
    match value {
        NotableEvent::StrongboxOpening {
            between,
            opening: opened,
        } => opening(between, opened),
        NotableEvent::Moved {
            between,
            name,
            units,
            amount: moved,
            from,
            to,
            move_kind,
        } => MeasuredEvent::Moved {
            between: span(between),
            name: name.clone(),
            units: *units,
            amount: amount(*moved),
            from: place(from),
            to: place(to),
            move_kind: match move_kind {
                MoveKind::Listing => MeasuredMoveKind::Listing,
                MoveKind::ListingReturned => MeasuredMoveKind::ListingReturned,
                MoveKind::Transfer => MeasuredMoveKind::Transfer,
            },
        },
        NotableEvent::Appeared {
            between,
            key,
            units,
            value,
        } => MeasuredEvent::Appeared {
            between: span(between),
            item: holding(key),
            units: *units,
            value: amount(*value),
        },
        NotableEvent::Left {
            between,
            key,
            units,
            value,
        } => MeasuredEvent::Left {
            between: span(between),
            item: holding(key),
            units: *units,
            value: amount(*value),
        },
        NotableEvent::Sale {
            between,
            items,
            amount: sold,
        } => MeasuredEvent::Sale {
            between: span(between),
            items: items
                .iter()
                .map(|line| MeasuredEventItem {
                    item: holding(&line.key),
                    units: line.units,
                    amount: amount(line.amount),
                })
                .collect(),
            amount: amount(*sold),
        },
        NotableEvent::Purchase {
            between,
            items,
            amount: bought,
        } => MeasuredEvent::Purchase {
            between: span(between),
            items: items
                .iter()
                .map(|line| MeasuredEventItem {
                    item: holding(&line.key),
                    units: line.units,
                    amount: amount(line.amount),
                })
                .collect(),
            amount: amount(*bought),
        },
        NotableEvent::ShrapnelConverted {
            between,
            shrapnel_units,
            shrapnel,
            ammo_units,
            ammo,
        } => MeasuredEvent::ShrapnelConverted {
            between: span(between),
            shrapnel_units: *shrapnel_units,
            shrapnel: amount(*shrapnel),
            ammo_units: *ammo_units,
            ammo: amount(*ammo),
        },
        NotableEvent::ListingSold {
            between,
            key,
            units,
            amount: sold,
        } => MeasuredEvent::ListingSold {
            between: span(between),
            item: holding(key),
            units: *units,
            amount: amount(*sold),
        },
        NotableEvent::CardChanged { between, from, to } => MeasuredEvent::CardChanged {
            between: span(between),
            from: card(*from),
            to: card(*to),
        },
    }
}

fn consumption(item: &ItemUnits) -> MeasuredConsumption {
    MeasuredConsumption {
        item: holding(&item.key),
        role: role(item.role),
        units: item.units,
        amount: amount(item.amount),
    }
}

fn session_measure(
    session_id: String,
    measure: &SessionMeasure,
    sealed: bool,
) -> Result<AccountSessionMeasure, ApiError> {
    let by_category =
        MeasureTotals::of(measure).map_err(account_error("account measure totals"))?;
    Ok(AccountSessionMeasure {
        session_id,
        sealed,
        coverage: coverage(measure),
        ammo: measure
            .ammo
            .iter()
            .map(|used| MeasuredAmmo {
                name: used.name.clone(),
                net_units: used.net_units,
                net: amount(used.net),
                split: match &used.split {
                    AmmoSplit::Settled {
                        burned_units,
                        burned,
                        strongbox_units,
                        strongbox,
                        converted_units,
                        converted,
                    } => MeasuredAmmoSplit::Settled {
                        burned_units: *burned_units,
                        burned: amount(*burned),
                        strongbox_units: *strongbox_units,
                        strongbox: amount(*strongbox),
                        converted_units: *converted_units,
                        converted: amount(*converted),
                    },
                    AmmoSplit::Unsettled {
                        burned_units_by_rows,
                        strongbox_units_by_rows,
                        converted_units,
                    } => MeasuredAmmoSplit::Unsettled {
                        burned_units_by_rows: *burned_units_by_rows,
                        strongbox_units_by_rows: *strongbox_units_by_rows,
                        converted_units: *converted_units,
                    },
                },
            })
            .collect(),
        decay: measure
            .decay
            .iter()
            .map(|item| MeasuredDecay {
                item: holding(&item.key),
                role: role(item.role),
                decay: amount(item.decay),
            })
            .collect(),
        repairs: measure
            .repairs
            .iter()
            .map(|timed| MeasuredRepair {
                between: span(&timed.between),
                items: timed
                    .repair
                    .items
                    .iter()
                    .map(|item| MeasuredItemAmount {
                        item: holding(&item.key),
                        amount: amount(item.amount),
                    })
                    .collect(),
                restored: amount(timed.repair.restored),
                cost: match timed.repair.cost {
                    RepairCost::Paid { pec } => MeasuredRepairCost::Paid { amount: card(pec) },
                    RepairCost::Derived { amount: cost } => MeasuredRepairCost::Derived {
                        amount: amount(cost),
                    },
                    RepairCost::Unknown => MeasuredRepairCost::Unknown,
                },
            })
            .collect(),
        other_consumption: measure.other_consumption.iter().map(consumption).collect(),
        unexplained_departures: measure
            .unexplained_departures
            .iter()
            .map(consumption)
            .collect(),
        gained: measure
            .gained
            .iter()
            .map(|gain| MeasuredGain {
                item: holding(&gain.key),
                role: role(gain.role),
                stacking: match gain.stacking {
                    Stacking::Stack => MeasuredStacking::Stack,
                    Stacking::Single => MeasuredStacking::Single,
                },
                mark: match gain.mark {
                    GainMark::Arrived => MeasuredGainMark::Arrived,
                    GainMark::PossiblyStrongbox => MeasuredGainMark::PossiblyStrongbox,
                    GainMark::CardMovedUnexplained => MeasuredGainMark::CardMovedUnexplained,
                },
                units: gain.units,
                amount: amount(gain.amount),
            })
            .collect(),
        gained_total: amount(measure.gained_total),
        gained_possibly_strongbox: amount(measure.gained_possibly_strongbox),
        events: measure.events.iter().map(event).collect(),
        totals: totals(&by_category),
    })
}

/// What the ammo across a session's types burned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MeasuredAmmoTotal {
    Settled {
        burned: MeasuredAmount,
    },
    /// An opening did not settle, so the total burned is not known.
    Unsettled,
}

/// A session's measure, by category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredTotals {
    pub ammo: MeasuredAmmoTotal,
    /// Weapons and their amplifiers.
    pub weapon_decay: MeasuredAmount,
    pub healing_decay: MeasuredAmount,
    /// Armour and its plates.
    pub armour_decay: MeasuredAmount,
    /// Everything else that wore.
    pub other_equipment_decay: MeasuredAmount,
    pub other_consumption: MeasuredAmount,
    /// Stacks that left with nothing to explain them: never a cost.
    pub unexplained_departures: MeasuredAmount,
    /// What the repairs whose cost the PED Card's fall shows alone cost.
    pub repair_cost: MeasuredAmount,
    /// What the repairs beside a sale or a purchase cost, as what remains
    /// of the Card's change.
    pub repair_cost_derived: MeasuredAmount,
    /// Repairs whose cost could not be read, left out of `repairCost`.
    pub repairs_uncosted: i64,
    pub gained: MeasuredAmount,
    pub gained_possibly_strongbox: MeasuredAmount,
}

fn totals(value: &MeasureTotals) -> MeasuredTotals {
    MeasuredTotals {
        ammo: match value.ammo {
            AmmoTotal::Settled(burned) => MeasuredAmmoTotal::Settled {
                burned: amount(burned),
            },
            AmmoTotal::Unsettled => MeasuredAmmoTotal::Unsettled,
        },
        weapon_decay: amount(value.weapon_decay),
        healing_decay: amount(value.healing_decay),
        armour_decay: amount(value.armour_decay),
        other_equipment_decay: amount(value.other_decay),
        other_consumption: amount(value.other_consumption),
        unexplained_departures: amount(value.unexplained_departures),
        repair_cost: amount(value.repairs.known),
        repair_cost_derived: amount(value.repairs.derived),
        repairs_uncosted: count(value.repairs.unknown),
        gained: amount(value.gained),
        gained_possibly_strongbox: amount(value.gained_possibly_strongbox),
    }
}

/// A session's tracked figures, as its summary holds them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackedSessionFigures {
    pub weapon_cost: f64,
    pub healing_cost: f64,
    pub armour_cost: f64,
    /// The session has hits no armour recording covers yet: its armour
    /// cost is not recorded, and `armourCost` is not a figure to show.
    pub armour_unrecorded: bool,
    pub loot_tt: f64,
    pub total_cycled: f64,
}

fn tracked(value: &TrackedFigures, armour_unrecorded: bool) -> TrackedSessionFigures {
    TrackedSessionFigures {
        weapon_cost: value.weapon_cost,
        healing_cost: value.heal_cost,
        armour_cost: value.armour_cost,
        armour_unrecorded,
        loot_tt: value.loot_tt,
        total_cycled: value.cycled,
    }
}

/// One measured session on a page, beside its tracked figures.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredSessionRow {
    pub id: String,
    pub name: Nullable<String>,
    pub start_time: String,
    pub end_time: Nullable<String>,
    pub coverage: MeasuredCoverageLevel,
    pub reading_count: i64,
    pub gap_count: i64,
    pub sealed: bool,
    /// `null` while the session counts in every stats scope; an excluded
    /// session keeps its readings and its measure.
    pub exclusion: Nullable<SessionExclusion>,
    pub measured: MeasuredTotals,
    /// `null` while the session has no summary.
    pub tracked: Nullable<TrackedSessionFigures>,
}

/// A keyset page of measured sessions, newest first, with the cursor for
/// the next page (`null` on the last) and how many sessions have readings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredSessionPage {
    pub sessions: Vec<MeasuredSessionRow>,
    pub next_cursor: Nullable<String>,
    pub total: i64,
}

fn row(
    session: &MeasuredSession,
    armour_unrecorded: &std::collections::HashSet<String>,
) -> Result<MeasuredSessionRow, ApiError> {
    let exclusion = eo_services::tracking_reads::exclusion_json(&session.exclusion);
    Ok(MeasuredSessionRow {
        id: session.id.clone(),
        name: session.name.clone().into(),
        start_time: iso(session.started_at),
        end_time: session.ended_at.map(iso).into(),
        coverage: coverage_level(session.coverage),
        reading_count: count(session.reading_count),
        gap_count: count(session.gap_count),
        sealed: session.sealed,
        exclusion: serde_json::from_value(exclusion)
            .map_err(ApiError::internal("measured session exclusion"))?,
        measured: totals(&session.totals),
        tracked: session
            .tracked
            .as_ref()
            .map(|figures| tracked(figures, armour_unrecorded.contains(&session.id)))
            .into(),
    })
}

/// One category summed across sessions: the tracked figure beside the
/// measured one, over the sessions where both are known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredSummaryPair {
    /// How many sessions went into this pair.
    pub sessions: i64,
    pub tracked: f64,
    pub measured: MeasuredAmount,
}

/// A category only the readings measure, summed across sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredSummaryOnly {
    pub sessions: i64,
    pub measured: MeasuredAmount,
}

/// Why a session with readings is left out of the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MeasuredLeftOutReason {
    PartialCoverage,
    /// Excluded from the economy statistics.
    Excluded,
    /// Its measure is not sealed yet.
    NotSealed,
}

/// How many sessions were left out for one reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredLeftOut {
    pub reason: MeasuredLeftOutReason,
    pub sessions: i64,
}

/// The whole history of measured sessions beside the tracked figures:
/// every session whose sealed measure covers it whole and that counts in
/// the economy statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountMeasuredSummary {
    /// Tracked weapon cost beside measured ammo burned plus weapon and
    /// amplifier wear; a session whose ammo split is unsettled is left out.
    pub weapon: MeasuredSummaryPair,
    /// Tracked healing cost beside measured healing wear.
    pub healing: MeasuredSummaryPair,
    /// Tracked armour cost beside measured armour and plate wear; a session
    /// whose armour is not recorded yet is left out.
    pub armour: MeasuredSummaryPair,
    /// Other equipment wear and other consumption: measured only.
    pub other: MeasuredSummaryOnly,
    /// Tracked loot TT beside what the readings saw arrive.
    pub loot: MeasuredSummaryPair,
    /// Sessions that went into the summary.
    pub included: i64,
    /// Sessions with readings that were left out, for every reason.
    pub left_out: Vec<MeasuredLeftOut>,
}

fn summary_pair(pair: &SummaryPair) -> MeasuredSummaryPair {
    MeasuredSummaryPair {
        sessions: count(pair.sessions),
        tracked: pair.tracked,
        measured: amount(pair.measured),
    }
}

pub(crate) fn account_error(context: &'static str) -> impl FnOnce(AccountError) -> ApiError {
    ApiError::internal(context)
}

impl Api {
    /// Attach the account connection (none when it could not be built).
    pub fn with_account(mut self, account: impl Into<Option<Arc<AccountRecorder>>>) -> Self {
        self.account = account.into();
        self
    }

    pub(crate) fn account(&self) -> Result<&Arc<AccountRecorder>, ApiError> {
        self.account
            .as_ref()
            .ok_or_else(|| ApiError::invalid_state("the account connection was not composed"))
    }

    /// The account connection, for the shell to hand it the session the
    /// player signed in with (Rust only: no command carries a session).
    /// `None` when it could not be set up.
    pub fn account_recorder(&self) -> Option<Arc<AccountRecorder>> {
        self.account.clone()
    }

    /// End the connection task as the app closes, waiting briefly.
    pub async fn account_shutdown(&self) {
        if let Some(account) = &self.account {
            account.shutdown().await;
        }
    }

    /// The connection's state. Nothing secret.
    pub async fn account_status(&self) -> Result<AccountStatus, ApiError> {
        let status = self.account()?.status();
        let state = wire_state(status.state);
        Ok(AccountStatus {
            state,
            signal: AccountSignal::of(state, status.has_connected),
            connected_at: status.connected_at.map(iso).into(),
            last_reading_at: status.last_reading_at.map(iso).into(),
            reading_session: status.reading_session.is_some(),
            has_connected: status.has_connected,
            confirming: status
                .confirming
                .map(|confirming| match confirming {
                    Confirming::Checking => AccountConfirming::Checking,
                    Confirming::SiteUnreachable => AccountConfirming::SiteUnreachable,
                })
                .into(),
            signing_in: status.signing_in,
            has_readings: status.has_readings,
        })
    }

    /// The connection at a glance, and nothing more: the one account fact
    /// every trusted window may read, so the overlay can show whether the
    /// account is being read.
    pub async fn account_signal(&self) -> Result<AccountSignal, ApiError> {
        let status = self.account()?.status();
        Ok(AccountSignal::of(
            wire_state(status.state),
            status.has_connected,
        ))
    }

    /// Drop the session and stop reading. The shell's disconnect closes
    /// the sign-in window and forgets the stored session before the
    /// connection drops it (it does not go through this method).
    pub async fn account_disconnect(&self) -> Result<(), ApiError> {
        self.account()?.disconnect().await;
        Ok(())
    }

    /// One tracked session's measure; `null` when the session has no
    /// readings, not found when there is no such session.
    pub async fn account_session_measure(
        &self,
        session_id: String,
    ) -> Result<Nullable<AccountSessionMeasure>, ApiError> {
        let read = self
            .account()?
            .session_measure(session_id.clone())
            .await
            .map_err(account_error("account session measure"))?;
        match read {
            MeasureRead::UnknownSession => Err(ApiError::not_found("Session not found")),
            MeasureRead::NoReadings => Ok(Nullable(None)),
            MeasureRead::Measured { measure, sealed } => Ok(Nullable(Some(session_measure(
                session_id, &measure, sealed,
            )?))),
        }
    }

    /// One keyset page of the tracked sessions with readings, newest
    /// first, each beside its tracked figures. A malformed cursor is a bad
    /// request.
    pub async fn account_measured_sessions(
        &self,
        cursor: Option<String>,
        limit: Option<i64>,
    ) -> Result<MeasuredSessionPage, ApiError> {
        let seek = match cursor.as_deref() {
            None => None,
            Some(token) => match eo_services::keyset::decode_cursor::<(f64, String)>(token) {
                Some(key) => Some(key),
                None => return Err(ApiError::bad_request("Invalid cursor")),
            },
        };
        let page = self
            .account()?
            .measured_sessions(seek, limit)
            .await
            .map_err(account_error("account measured sessions"))?;
        let ids: Vec<String> = page
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect();
        let unrecorded = self.armour_unrecorded(ids).await?;
        Ok(MeasuredSessionPage {
            sessions: page
                .sessions
                .iter()
                .map(|session| row(session, &unrecorded))
                .collect::<Result<_, _>>()?,
            next_cursor: page
                .next
                .map(|key| eo_services::keyset::encode_cursor(&key))
                .into(),
            total: page.total,
        })
    }

    /// Which of `session_ids` have hits no armour recording covers yet: the
    /// same reading the session detail marks "not recorded yet" from.
    async fn armour_unrecorded(
        &self,
        session_ids: Vec<String>,
    ) -> Result<std::collections::HashSet<String>, ApiError> {
        if session_ids.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        Ok(self
            .protection_unrecorded_sessions(session_ids)
            .await?
            .into_iter()
            .collect())
    }

    /// The whole history of measured sessions, by category, beside the
    /// tracked figures (see [`AccountMeasuredSummary`]). A cross-session
    /// statistic: an excluded session is left out exactly as the other
    /// statistics leave it out. A read of cached measures only: it
    /// computes and writes nothing.
    pub async fn account_measured_summary(&self) -> Result<AccountMeasuredSummary, ApiError> {
        let ids = self
            .db
            .with_reader(|conn| Ok(sessions::sessions_with_readings(conn)))
            .await
            .map_err(ApiError::internal("account measured summary"))?
            .map_err(account_error("account measured summary"))?;
        let unrecorded = self.armour_unrecorded(ids).await?;
        let summary = self
            .db
            .with_reader(move |conn| Ok(sessions::measured_summary(conn, &unrecorded)))
            .await
            .map_err(ApiError::internal("account measured summary"))?
            .map_err(account_error("account measured summary"))?;
        Ok(AccountMeasuredSummary {
            weapon: summary_pair(&summary.weapon),
            healing: summary_pair(&summary.healing),
            armour: summary_pair(&summary.armour),
            other: MeasuredSummaryOnly {
                sessions: count(summary.other.sessions),
                measured: amount(summary.other.measured),
            },
            loot: summary_pair(&summary.loot),
            included: count(summary.included),
            left_out: summary
                .left_out
                .iter()
                .map(|(reason, sessions)| MeasuredLeftOut {
                    reason: match reason {
                        LeftOutReason::PartialCoverage => MeasuredLeftOutReason::PartialCoverage,
                        LeftOutReason::Excluded => MeasuredLeftOutReason::Excluded,
                        LeftOutReason::NotSealed => MeasuredLeftOutReason::NotSealed,
                    },
                    sessions: count(*sessions),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{AccountConnectionState as State, AccountSignal as Signal};

    #[test]
    fn every_state_reads_as_one_signal() {
        let cases = [
            (State::Connected, true, Signal::SignedIn),
            (State::Expired, true, Signal::SignedOut),
            (State::Unconfirmed, true, Signal::Checking),
            // At launch, a player who connected before is simply not
            // signed in yet; one who never connected sees nothing.
            (State::Disconnected, true, Signal::NotSignedIn),
            (State::Disconnected, false, Signal::Off),
        ];
        for (state, has_connected, signal) in cases {
            assert_eq!(Signal::of(state, has_connected), signal, "{state:?}");
        }
    }
}
