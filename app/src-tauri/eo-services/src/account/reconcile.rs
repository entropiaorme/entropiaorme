//! The account's side of a stock reconciliation: what the latest reading,
//! or an inventory the player pasted from the account site, shows of the
//! player's stock, as the plain observation the analytics service compares
//! with what it holds (see [`crate::analytics::AnalyticsService::plan_reconciliation`]).
//!
//! A reading and a paste of the same inventory are read into the same
//! holdings ([`super::paste`]), so they give the same plan. What counts is
//! loose stock: rows on the avatar, in storage, or in a place the app does
//! not know, merged by name across them as the account site merges them.
//! Held back, with the reason shown:
//!
//! - Universal Ammo, which is liquid PED;
//! - equipment, armour and plates, by the role the readings give them;
//! - rows attached to another item, and rows listed at auction;
//! - non-Universal ammo;
//! - consumables, which are used up as a cost, and strongboxes and their
//!   keys, which the opening flow records, unless the app holds them as
//!   stock all the same;
//! - every item a pending proposal names, until the player decides it.
//!
//! The comparison itself, the draft and every write belong to the
//! analytics service; nothing here writes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::paste::{holdings_from_paste, PasteRefusal};
use super::proposals::{self, Detail};
use super::reading::{Container, Reading};
use super::roles::{
    is_strongbox, known_category, CostCategory, KnownRoles, Role, RoleBook, RoleSource, Stacking,
    STRONGBOX_KEY, UNIVERSAL_AMMO,
};
use super::store;
use super::AccountError;
use crate::analytics::{
    reconciliation_key, AnalyticsError, AnalyticsService, ExcludedItem, Exclusion, Observation,
    ObservedStack, ReconciliationDraft, ReconciliationSource,
};
use crate::db::Db;
use crate::game_data_store::GameDataStore;

/// Why a reconciliation could not be planned.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// No reading has been stored to plan from.
    #[error("no account reading has been stored yet")]
    NoReading,
    /// The pasted inventory does not read.
    #[error("{}", .0.describe())]
    Paste(PasteRefusal),
    #[error(transparent)]
    Account(#[from] AccountError),
    #[error(transparent)]
    Analytics(#[from] AnalyticsError),
}

/// Every item a pending proposal names, by the name a reconciliation
/// compares under.
pub fn pending_items(conn: &rusqlite::Connection) -> Result<BTreeSet<String>, AccountError> {
    let mut names = BTreeSet::new();
    for proposal in proposals::pending(conn, None)? {
        match &proposal.detail {
            Detail::Opening(opening) => {
                names.insert(reconciliation_key(STRONGBOX_KEY));
                for boxes in &opening.boxes {
                    names.insert(reconciliation_key(&boxes.name));
                }
                for content in opening.contents.iter().chain(&opening.possible_contents) {
                    names.insert(reconciliation_key(&content.key.name));
                }
            }
            Detail::Listing(listing) => {
                names.insert(reconciliation_key(&listing.name));
            }
            Detail::Purchase(purchase) => {
                names.insert(reconciliation_key(&purchase.key.name));
            }
            Detail::Arrival(arrival) => {
                names.insert(reconciliation_key(&arrival.key.name));
            }
        }
    }
    names.remove(UNIVERSAL_AMMO);
    Ok(names)
}

/// How one holding of the reading takes part.
enum Taking {
    /// Compared with the app's stock.
    Compared,
    /// Compared only when the app holds the item as stock, held back for
    /// this reason otherwise.
    UnlessHeld(Exclusion),
    /// Held back for this reason.
    Excluded(Exclusion),
}

/// How one holding of the reading takes part in a reconciliation.
fn taking(
    name: &str,
    container: &Container,
    book: &RoleBook,
    roles: &dyn RoleSource,
    pending: &BTreeSet<String>,
) -> Taking {
    let trimmed = name.trim();
    if trimmed == UNIVERSAL_AMMO {
        return Taking::Excluded(Exclusion::UniversalAmmo);
    }
    match container {
        Container::Auction => return Taking::Excluded(Exclusion::Auction),
        Container::Attached { .. } => return Taking::Excluded(Exclusion::Attached),
        Container::Carried | Container::Storage { .. } | Container::Other { .. } => {}
    }
    if pending.contains(&reconciliation_key(trimmed)) {
        return Taking::Excluded(Exclusion::PendingProposal);
    }
    // Strongboxes and their keys are the opening flow's, and consumables
    // are used up as a cost; either is compared only where the app holds it
    // as stock all the same.
    if trimmed == STRONGBOX_KEY || is_strongbox(trimmed) {
        return Taking::UnlessHeld(Exclusion::Strongbox);
    }
    match book.role(name) {
        Role::Ammo => Taking::Excluded(Exclusion::Ammo),
        Role::Weapon | Role::Amplifier | Role::Healing | Role::OtherEquipment => {
            Taking::Excluded(Exclusion::Equipment)
        }
        Role::Armour => Taking::Excluded(Exclusion::Armour),
        Role::Plating => Taking::Excluded(Exclusion::Plate),
        Role::Material => {
            let consumable = roles
                .placed_category(trimmed)
                .or_else(|| known_category(roles, trimmed))
                .is_some_and(CostCategory::is_consumable);
            if consumable {
                Taking::UnlessHeld(Exclusion::Consumable)
            } else {
                Taking::Compared
            }
        }
    }
}

/// The game data's TT for one unit of a stackable it lists (a stimulant or
/// an enhancer): a constant of the game, so exact.
fn catalogue_unit_tt(game_data: Option<&GameDataStore>, name: &str) -> Option<f64> {
    let data = game_data?;
    let name = name.trim();
    ["stimulants", "enhancers"].iter().find_map(|endpoint| {
        data.get_entities(endpoint)
            .iter()
            .find(|entity| {
                entity
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|listed| listed.trim() == name)
            })
            .and_then(|entity| entity.pointer("/economy/max_tt"))
            .and_then(serde_json::Value::as_f64)
            .filter(|tt| tt.is_finite() && *tt > 0.0)
    })
}

/// What one reading shows of the player's loose stock, merged by name,
/// beside what it holds back and why.
pub fn observe(
    reading: &Reading,
    source: ReconciliationSource,
    roles: &KnownRoles,
    pending: &BTreeSet<String>,
    game_data: Option<&GameDataStore>,
) -> Observation {
    #[derive(Default)]
    struct Stack {
        name: String,
        quantity: i64,
        pec: i64,
        rows: usize,
        single: bool,
        unless_held: Option<Exclusion>,
    }
    let book = RoleBook::resolve(roles, std::slice::from_ref(reading));
    let mut stacks: BTreeMap<String, Stack> = BTreeMap::new();
    let mut excluded: BTreeMap<(String, Exclusion), (String, i64)> = BTreeMap::new();
    for (key, holding) in reading.holdings() {
        let item_key = reconciliation_key(&key.name);
        let unless_held = match taking(&key.name, &key.container, &book, roles, pending) {
            Taking::Excluded(reason) => {
                let entry = excluded
                    .entry((item_key, reason))
                    .or_insert_with(|| (key.name.trim().to_string(), 0));
                entry.1 = entry.1.saturating_add(holding.quantity());
                continue;
            }
            Taking::UnlessHeld(reason) => Some(reason),
            Taking::Compared => None,
        };
        let stack = stacks.entry(item_key).or_insert_with(|| Stack {
            name: key.name.trim().to_string(),
            single: true,
            ..Stack::default()
        });
        stack.quantity = stack.quantity.saturating_add(holding.quantity());
        stack.pec = stack.pec.saturating_add(holding.value().0);
        stack.rows += holding.row_count();
        stack.single &= book.stacking(&key.name) == Stacking::Single;
        stack.unless_held = stack.unless_held.or(unless_held);
    }
    // An item a proposal waits on is held back even where the account
    // shows none of it, so what the app holds of it is not called missing.
    for name in pending {
        excluded
            .entry((name.clone(), Exclusion::PendingProposal))
            .or_insert_with(|| (name.clone(), 0));
    }
    Observation {
        source,
        observed_at: reading.taken_at(),
        stacks: stacks
            .into_values()
            .map(|stack| ObservedStack {
                catalogue_unit_tt: catalogue_unit_tt(game_data, &stack.name),
                item_name: stack.name,
                quantity: stack.quantity,
                tt_value: stack.pec as f64 / 100.0,
                rows: stack.rows,
                single: stack.single,
                unless_held: stack.unless_held,
            })
            .collect(),
        excluded: excluded
            .into_iter()
            .map(|((_, reason), (item_name, quantity))| ExcludedItem {
                item_name,
                quantity: quantity as f64,
                reason,
            })
            .collect(),
    }
}

/// The player's roles and the items pending proposals name, read once.
async fn known(
    db: &Db,
    game_data: Option<Arc<GameDataStore>>,
) -> Result<(KnownRoles, BTreeSet<String>), AccountError> {
    db.with_reader(move |conn| {
        let known = KnownRoles::read(conn, game_data.as_deref())
            .and_then(|roles| Ok((roles, pending_items(conn)?)));
        Ok(known)
    })
    .await
    .map_err(AccountError::from)?
}

/// Plan a reconciliation from the latest stored reading, and keep it as the
/// draft being reviewed.
pub async fn plan_from_reading(
    db: &Db,
    analytics: &AnalyticsService,
    game_data: Option<Arc<GameDataStore>>,
) -> Result<ReconciliationDraft, PlanError> {
    let latest = db
        .with_reader(|conn| Ok(store::latest_reading(conn)))
        .await
        .map_err(AccountError::from)??;
    let Some(latest) = latest else {
        return Err(PlanError::NoReading);
    };
    let (roles, pending) = known(db, game_data.clone()).await?;
    let observation = observe(
        &latest.reading,
        ReconciliationSource::Reading {
            reading_id: latest.header.id.0,
        },
        &roles,
        &pending,
        game_data.as_deref(),
    );
    Ok(analytics.plan_reconciliation(observation).await?)
}

/// Plan a reconciliation from an inventory pasted from the account site,
/// observed at `now` (unix seconds), and keep it as the draft being
/// reviewed. Works without a connection.
pub async fn plan_from_paste(
    db: &Db,
    analytics: &AnalyticsService,
    game_data: Option<Arc<GameDataStore>>,
    text: &str,
    now: f64,
) -> Result<ReconciliationDraft, PlanError> {
    let holdings = holdings_from_paste(text).map_err(PlanError::Paste)?;
    let reading = Reading::new(now, super::amount::Pec::ZERO, holdings)?;
    let (roles, pending) = known(db, game_data.clone()).await?;
    let observation = observe(
        &reading,
        ReconciliationSource::Paste,
        &roles,
        &pending,
        game_data.as_deref(),
    );
    Ok(analytics.plan_reconciliation(observation).await?)
}
