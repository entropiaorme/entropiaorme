//! What an item in a reading is, for grouping the measure and costing it.
//!
//! A role only groups: every measured figure is also kept per item, so a
//! wrong role shows as an item in the wrong group rather than vanishing.
//! A role is the coarse face of a finer [`CostCategory`], which says what
//! an item costs as (a weapon's attachment, a limited armour piece, a
//! teleport chip); both come from one resolution. What a measured use of an
//! item costs as answers first from the player's own placement of it (see
//! [`super::placement`]), which the player makes once for an item nothing else
//! places, and which outranks everything below: it decides what the use
//! costs as, never how the readings group the item. Otherwise a name's
//! category is the first answer from, in order:
//!
//! 1. the player's equipment library (an entry's own name, and the
//!    amplifier, implant and attachments a weapon entry carries);
//! 2. the bundled game data (the weapon, amplifier, medical tool,
//!    attachment, absorber, implant, harvesting tool, stimulant and
//!    enhancer collections);
//! 3. the item's own name: Universal Ammo is ammo, a name starting
//!    `Armor Plating` is plating, `Mind Essence` is the mindforce fuel, a
//!    `Teleportation Chip` is one, and the portable repair and TT units are
//!    field logistics;
//! 4. the player's protection sets, whose names name a family of pieces
//!    (`Pegasus` names `Pegasus Helmet (M,L)`);
//! 5. how the reading holds it: the parent of an attached plate is armour,
//!    and an item attached to a weapon is its amplifier or an attachment;
//! 6. otherwise, a single item that wears is other equipment, and anything
//!    else is a material; neither has a category, so both cost as
//!    unclassified.
//!
//! Exact names (the library and the game data) answer before the
//! protection sets' family names, so a set named after a maker cannot
//! claim that maker's weapons. Armour and plates are limited when their
//! name's tag says so (`(L)`, `(M,L)`) or their set is a limited one.
//! Resolution only reads: nothing here writes the library, the sets, or
//! the game data.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::reading::{Container, HoldingKey, Reading};
use super::AccountError;
use crate::bundled_tools::entity_name;
use crate::consumables::consumable_profile_from_props;
use crate::game_data_store::GameDataStore;
use crate::protection::ProtectionSetKind;

/// The one ammunition every weapon and chip now fires.
pub const UNIVERSAL_AMMO: &str = "Universal Ammo";

/// The metal scrap kills leave, which the Trade Terminal turns into
/// Universal Ammo.
pub const SHRAPNEL: &str = "Shrapnel";

/// The key every strongbox takes, as the game names it.
pub use crate::inventory_changes::STRONGBOX_KEY;

/// Whether a name is a strongbox's: it ends in `Strongbox`.
pub fn is_strongbox(name: &str) -> bool {
    name.trim().to_ascii_lowercase().ends_with("strongbox")
}

/// The prefix every armour plate's name carries.
const PLATING_PREFIX: &str = "Armor Plating";

/// The fuel mindforce chips burn.
pub const MIND_ESSENCE: &str = "Mind Essence";

/// The prefix every teleportation chip's name carries.
const TELEPORT_CHIP_PREFIX: &str = "Teleportation Chip";

/// The prefixes of the convenience tools carried into the field: a repair
/// unit and a Trade Terminal unit of one's own.
const FIELD_LOGISTICS_PREFIXES: &[&str] = &["Portable Repair Unit", "Portable TT Unit"];

/// The game-data collections that name a category, in the order a name
/// found in two of them is settled.
const CATALOGUE_CATEGORIES: &[(&str, CostCategory)] = &[
    ("weapons", CostCategory::Weapon),
    ("weapon_amplifiers", CostCategory::Amplifier),
    ("medical_tools", CostCategory::HealingTool),
    ("weapon_vision_attachments", CostCategory::Attachment),
    ("absorbers", CostCategory::Attachment),
    ("mindforce_implants", CostCategory::MindforceImplant),
    ("harvesting_tools", CostCategory::HarvestingTool),
    ("stimulants", CostCategory::Consumable),
    ("enhancers", CostCategory::Enhancer),
];

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Ammo,
    Weapon,
    Amplifier,
    Healing,
    Armour,
    Plating,
    OtherEquipment,
    Material,
}

/// What an item costs as when it is used: the finer category of which a
/// [`Role`] is the coarse face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CostCategory {
    Ammo,
    Weapon,
    Amplifier,
    /// Something a weapon carries besides its amplifier: a scope, a sight,
    /// an absorber or an extender.
    Attachment,
    MindforceImplant,
    Enhancer,
    HarvestingTool,
    HealingTool,
    /// A healing item used up by the unit.
    HealingConsumable,
    UnlimitedArmour,
    UnlimitedPlate,
    LimitedArmour,
    LimitedPlate,
    /// A stimulant or another dose taken by the unit.
    Consumable,
    TeleportChip,
    /// The fuel mindforce chips burn.
    MindEssence,
    /// A convenience tool carried into the field: a portable repair or
    /// Trade Terminal unit.
    FieldLogistics,
    /// An item the player placed as travel: a vehicle, its fuel.
    Travel,
    /// Nothing the app knows names it.
    Unclassified,
    /// An item the player placed as no cost at all: its use costs nothing
    /// and is listed nowhere.
    NotACost,
}

impl CostCategory {
    pub const ALL: [CostCategory; 20] = [
        CostCategory::Ammo,
        CostCategory::Weapon,
        CostCategory::Amplifier,
        CostCategory::Attachment,
        CostCategory::MindforceImplant,
        CostCategory::Enhancer,
        CostCategory::HarvestingTool,
        CostCategory::HealingTool,
        CostCategory::HealingConsumable,
        CostCategory::UnlimitedArmour,
        CostCategory::UnlimitedPlate,
        CostCategory::LimitedArmour,
        CostCategory::LimitedPlate,
        CostCategory::Consumable,
        CostCategory::TeleportChip,
        CostCategory::MindEssence,
        CostCategory::FieldLogistics,
        CostCategory::Travel,
        CostCategory::Unclassified,
        CostCategory::NotACost,
    ];

    /// The categories the player may place an item in: every Cycled one the
    /// costs know but ammo (read from its own split), an enhancer (costed
    /// from the chat log) and limited armour (priced through its set's
    /// confirmed pieces); travel; field logistics; and no cost.
    pub const PLACEABLE: [CostCategory; 13] = [
        CostCategory::Weapon,
        CostCategory::Amplifier,
        CostCategory::Attachment,
        CostCategory::MindforceImplant,
        CostCategory::HarvestingTool,
        CostCategory::HealingTool,
        CostCategory::HealingConsumable,
        CostCategory::UnlimitedArmour,
        CostCategory::UnlimitedPlate,
        CostCategory::Consumable,
        CostCategory::Travel,
        CostCategory::FieldLogistics,
        CostCategory::NotACost,
    ];

    /// Whether the player may place an item in this category.
    pub fn is_placeable(self) -> bool {
        CostCategory::PLACEABLE.contains(&self)
    }

    /// The coarse role an item of this category plays in the measure.
    pub fn role(self) -> Role {
        match self {
            CostCategory::Ammo => Role::Ammo,
            CostCategory::Weapon => Role::Weapon,
            CostCategory::Amplifier => Role::Amplifier,
            CostCategory::HealingTool => Role::Healing,
            CostCategory::UnlimitedArmour | CostCategory::LimitedArmour => Role::Armour,
            CostCategory::UnlimitedPlate | CostCategory::LimitedPlate => Role::Plating,
            CostCategory::Attachment
            | CostCategory::MindforceImplant
            | CostCategory::HarvestingTool
            | CostCategory::TeleportChip
            | CostCategory::FieldLogistics
            | CostCategory::Travel => Role::OtherEquipment,
            CostCategory::Enhancer
            | CostCategory::HealingConsumable
            | CostCategory::Consumable
            | CostCategory::MindEssence
            | CostCategory::Unclassified
            | CostCategory::NotACost => Role::Material,
        }
    }

    /// Whether a stack of this category is used up by the unit: the only
    /// stacks whose leaving is ever a cost. Ammo is used up too, but is
    /// read from its own split into burned and paid back. A stack the
    /// player placed as travel (fuel) is used up like Mind Essence.
    pub fn is_consumable(self) -> bool {
        matches!(
            self,
            CostCategory::HealingConsumable
                | CostCategory::Consumable
                | CostCategory::MindEssence
                | CostCategory::Travel
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CostCategory::Ammo => "ammo",
            CostCategory::Weapon => "weapon",
            CostCategory::Amplifier => "amplifier",
            CostCategory::Attachment => "attachment",
            CostCategory::MindforceImplant => "mindforceImplant",
            CostCategory::Enhancer => "enhancer",
            CostCategory::HarvestingTool => "harvestingTool",
            CostCategory::HealingTool => "healingTool",
            CostCategory::HealingConsumable => "healingConsumable",
            CostCategory::UnlimitedArmour => "unlimitedArmour",
            CostCategory::UnlimitedPlate => "unlimitedPlate",
            CostCategory::LimitedArmour => "limitedArmour",
            CostCategory::LimitedPlate => "limitedPlate",
            CostCategory::Consumable => "consumable",
            CostCategory::TeleportChip => "teleportChip",
            CostCategory::MindEssence => "mindEssence",
            CostCategory::FieldLogistics => "fieldLogistics",
            CostCategory::Travel => "travel",
            CostCategory::Unclassified => "unclassified",
            CostCategory::NotACost => "notACost",
        }
    }

    pub fn parse(value: &str) -> Option<CostCategory> {
        CostCategory::ALL
            .into_iter()
            .find(|category| category.as_str() == value)
    }

    /// Armour or a plate, limited when `limited` says so; any other
    /// category as it is.
    fn limited_when(self, limited: bool) -> CostCategory {
        match self {
            CostCategory::UnlimitedArmour if limited => CostCategory::LimitedArmour,
            CostCategory::UnlimitedPlate if limited => CostCategory::LimitedPlate,
            other => other,
        }
    }
}

/// Whether an item's name marks it limited: its closing tag, in brackets,
/// holds an `L` among its comma-separated marks (`(L)`, `(M,L)`).
pub fn name_is_limited(name: &str) -> bool {
    let Some(inner) = name.trim().strip_suffix(')') else {
        return false;
    };
    let Some(open) = inner.rfind('(') else {
        return false;
    };
    inner[open + 1..].split(',').any(|mark| mark.trim() == "L")
}

/// How a measured fact used an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    /// Its TT fell while it stayed one item.
    Wear,
    /// Units of a stack left.
    Units,
}

/// Whether an item's rows count units of a stackable or hold one item
/// each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stacking {
    /// Units of one stackable: a change is a quantity consumed or gained.
    Stack,
    /// One item per row: a change is an item wearing, restored, arriving
    /// or leaving.
    Single,
}

/// What the player chose for a consumable in their equipment library: the
/// choice that decides whether a dose of it books a cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DosePricing {
    /// Each dose books its TT at the item's recorded acquisition markup,
    /// in hundredths of a percent of TT.
    Booked { markup_hundredths: i64 },
    /// Doses book nothing: the player accounts for the item through the
    /// ledger, where its purchase is recorded.
    ThroughLedger,
}

/// Where category evidence comes from, so tests can stand one in. A role
/// is only ever read through the category, never apart from it.
pub trait RoleSource {
    /// The category the player's equipment library gives this exact name.
    fn library_category(&self, name: &str) -> Option<CostCategory>;
    /// The category the bundled game data gives this exact name.
    fn catalogue_category(&self, name: &str) -> Option<CostCategory>;
    /// The category a protection set's family name gives this name.
    fn protection_category(&self, name: &str) -> Option<CostCategory>;

    /// The role the player's equipment library gives this exact name.
    fn library_role(&self, name: &str) -> Option<Role> {
        self.library_category(name).map(CostCategory::role)
    }
    /// The role the bundled game data gives this exact name.
    fn catalogue_role(&self, name: &str) -> Option<Role> {
        self.catalogue_category(name).map(CostCategory::role)
    }
    /// The role a protection set's family name gives this name.
    fn protection_role(&self, name: &str) -> Option<Role> {
        self.protection_category(name).map(CostCategory::role)
    }
    /// What the player chose for this consumable in the library, when the
    /// library holds it as one.
    fn dose_pricing(&self, _name: &str) -> Option<DosePricing> {
        None
    }
    /// The category the player placed this exact name in, when they have.
    fn placed_category(&self, _name: &str) -> Option<CostCategory> {
        None
    }
    /// Whether the app has recorded this exact name as loot, or holds it as
    /// stock: what play produces and the player trades, never what the unit
    /// uses up.
    fn held_as_loot_or_stock(&self, _name: &str) -> bool {
        false
    }
}

/// A confirmed piece of a limited set: the set, and the markup its pieces
/// are billed at, in hundredths of a percent of TT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetPiece {
    pub set_id: i64,
    pub markup_hundredths: i64,
}

/// One confirmation of a piece as stored: the item's exact name, its set,
/// the set's markup in percent, when it was confirmed, and when it ended
/// (the piece removed, or its set taken out of use), if it has.
#[derive(Debug, Clone, PartialEq)]
pub struct PieceRow {
    pub item: String,
    pub set_id: i64,
    pub markup_percent: f64,
    pub confirmed_at: f64,
    pub removed_at: Option<f64>,
}

/// One confirmation of a piece, as resolution reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PieceSpan {
    piece: SetPiece,
    confirmed_at: f64,
    removed_at: Option<f64>,
}

/// The player's library and sets and the bundled game data, read once.
#[derive(Debug, Clone)]
pub struct KnownRoles {
    library: HashMap<String, CostCategory>,
    catalogue: HashMap<String, CostCategory>,
    /// Lower-cased family name and the category its pieces take.
    sets: Vec<(String, CostCategory)>,
    /// The library's consumables, by name, and how their doses book.
    doses: HashMap<String, DosePricing>,
    /// Every confirmation of each piece, by exact name, oldest first.
    pieces: HashMap<String, Vec<PieceSpan>>,
    /// The player's own placements, by exact name.
    placements: HashMap<String, CostCategory>,
    /// The names the app has recorded as loot or holds as stock.
    held: BTreeSet<String>,
    /// A digest of everything above: two readings of the same library,
    /// sets and game data have the same one, and any change gives another.
    fingerprint: String,
}

impl Default for KnownRoles {
    /// Nothing known: no library, no sets, no game data.
    fn default() -> KnownRoles {
        KnownRoles::from_parts(&[], &[], None)
    }
}

impl KnownRoles {
    /// Read the equipment library and the protection sets (archived ones
    /// too: they still name gear the player may hold), beside the bundled
    /// game data when there is one.
    pub fn read(
        conn: &rusqlite::Connection,
        game_data: Option<&GameDataStore>,
    ) -> Result<KnownRoles, AccountError> {
        let library = {
            let mut stmt =
                conn.prepare("SELECT name, item_type, properties_json FROM equipment_library")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let sets = {
            let mut stmt = conn.prepare("SELECT kind, name, economy_kind FROM protection_sets")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let placements = super::placement::read_all(conn)?
            .into_iter()
            .map(|placed| (placed.item, placed.category))
            .collect::<Vec<_>>();
        let pieces = {
            let mut stmt = conn.prepare(
                "SELECT p.item_name, p.set_id, s.markup_percent, p.confirmed_at, p.removed_at \
                 FROM protection_set_pieces p JOIN protection_sets s ON s.id = p.set_id \
                 WHERE s.economy_kind = 'limited' AND s.markup_percent IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(PieceRow {
                    item: row.get(0)?,
                    set_id: row.get(1)?,
                    markup_percent: row.get(2)?,
                    confirmed_at: row.get(3)?,
                    removed_at: row.get(4)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        // What play has produced (every loot item recorded, killed or
        // harvested) and what the player holds as stock now.
        let held = {
            let mut stmt = conn.prepare(
                "SELECT item_name FROM kill_loot_items \
                 UNION SELECT item_name FROM harvest_loot_items \
                 UNION SELECT item_name FROM \
                   (SELECT item_name FROM stock_movements GROUP BY item_name \
                    HAVING SUM(quantity) > 0)",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(
            KnownRoles::with_placements(&library, &sets, &pieces, &placements, game_data)
                .with_held(held),
        )
    }

    /// These roles, knowing `held` as the names the app has recorded as
    /// loot or holds as stock; the digest covers them, so a change to them
    /// shows the costs were placed by other roles.
    pub fn with_held(mut self, held: impl IntoIterator<Item = String>) -> KnownRoles {
        self.held = held
            .into_iter()
            .map(|name| name.trim().to_string())
            .collect();
        let mut hasher = Sha256::new();
        hasher.update(self.fingerprint.as_bytes());
        hasher.update(b"held");
        for name in &self.held {
            hasher.update((name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
        }
        self.fingerprint = hasher
            .finalize()
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self
    }

    /// Build from library rows `(name, item type, properties)`, protection
    /// sets `(kind, name, economy kind)`, and the game data.
    pub fn from_parts(
        library: &[(String, String, String)],
        sets: &[(String, String, String)],
        game_data: Option<&GameDataStore>,
    ) -> KnownRoles {
        KnownRoles::with_pieces(library, sets, &[], game_data)
    }

    /// [`KnownRoles::from_parts`], with every confirmation of the limited
    /// sets' pieces.
    pub fn with_pieces(
        library: &[(String, String, String)],
        sets: &[(String, String, String)],
        pieces: &[PieceRow],
        game_data: Option<&GameDataStore>,
    ) -> KnownRoles {
        KnownRoles::with_placements(library, sets, pieces, &[], game_data)
    }

    /// [`KnownRoles::with_pieces`], with the player's own placements
    /// `(exact name, category)`.
    pub fn with_placements(
        library: &[(String, String, String)],
        sets: &[(String, String, String)],
        pieces: &[PieceRow],
        placements: &[(String, CostCategory)],
        game_data: Option<&GameDataStore>,
    ) -> KnownRoles {
        let mut known = KnownRoles {
            library: HashMap::new(),
            catalogue: HashMap::new(),
            sets: Vec::new(),
            doses: HashMap::new(),
            pieces: HashMap::new(),
            placements: placements
                .iter()
                .map(|(item, category)| (item.trim().to_string(), *category))
                .collect(),
            held: BTreeSet::new(),
            fingerprint: String::new(),
        };
        for (name, item_type, properties) in library {
            known.add_library_entry(name, item_type, properties);
        }
        for (kind, name, economy) in sets {
            let category = if kind == ProtectionSetKind::Armour.as_str() {
                CostCategory::UnlimitedArmour
            } else if kind == ProtectionSetKind::Plates.as_str() {
                CostCategory::UnlimitedPlate
            } else {
                continue;
            };
            let family = name.trim().to_lowercase();
            if !family.is_empty() {
                known
                    .sets
                    .push((family, category.limited_when(economy == "limited")));
            }
        }
        if let Some(store) = game_data {
            for (endpoint, category) in CATALOGUE_CATEGORIES {
                for entity in store.get_entities(endpoint) {
                    if let Some(name) = entity_name(entity) {
                        known.catalogue.entry(name).or_insert(*category);
                    }
                }
            }
        }
        for row in pieces {
            known
                .pieces
                .entry(row.item.trim().to_string())
                .or_default()
                .push(PieceSpan {
                    piece: SetPiece {
                        set_id: row.set_id,
                        markup_hundredths: (row.markup_percent * 100.0).round() as i64,
                    },
                    confirmed_at: row.confirmed_at,
                    removed_at: row.removed_at,
                });
        }
        for spans in known.pieces.values_mut() {
            spans.sort_by(|a, b| {
                a.confirmed_at
                    .total_cmp(&b.confirmed_at)
                    .then(a.piece.set_id.cmp(&b.piece.set_id))
            });
        }
        known.fingerprint = fingerprint(library, sets, pieces, placements, &known.catalogue);
        known
    }

    /// The set a name is a piece of for a session that started at `at`:
    /// the earliest of its confirmations not ended by then. A session
    /// played before a piece was first confirmed belongs to that first
    /// confirmation, and one played after a confirmation ended to the next
    /// one made, so a piece that moves to a new set leaves the sessions
    /// billed under the old one with the old set.
    pub fn set_piece(&self, name: &str, at: f64) -> Option<SetPiece> {
        self.pieces
            .get(name.trim())?
            .iter()
            .find(|span| span.removed_at.is_none_or(|removed| removed > at))
            .map(|span| span.piece)
    }

    /// Every limited set some piece belongs to for a session that started
    /// at `at`, by id, with its markup in hundredths of a percent.
    pub fn confirmed_sets(&self, at: f64) -> BTreeMap<i64, i64> {
        self.pieces
            .keys()
            .filter_map(|name| self.set_piece(name, at))
            .map(|piece| (piece.set_id, piece.markup_hundredths))
            .collect()
    }

    /// The digest of the library, sets and game data these roles were read
    /// from: what a figure derived from them names, so a later change to
    /// any of them shows it was derived from other roles.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    fn add_library_entry(&mut self, name: &str, item_type: &str, properties: &str) {
        let own = match item_type {
            "weapon" => CostCategory::Weapon,
            "healing" => CostCategory::HealingTool,
            "tool" => CostCategory::HarvestingTool,
            "consumable" => CostCategory::Consumable,
            _ => return,
        };
        self.library.entry(name.trim().to_string()).or_insert(own);
        if own == CostCategory::Consumable {
            // The same reading of the player's choice the dose booking makes.
            let props = serde_json::from_str::<Value>(properties).unwrap_or(Value::Null);
            let profile = consumable_profile_from_props(&props, None).profile;
            let pricing = if profile.track_cost {
                DosePricing::Booked {
                    markup_hundredths: (profile.markup_percent * 100.0).round() as i64,
                }
            } else {
                DosePricing::ThroughLedger
            };
            self.doses.entry(name.trim().to_string()).or_insert(pricing);
        }
        if own != CostCategory::Weapon {
            return;
        }
        // A weapon entry names the amplifier, implant and attachments it
        // is used with; each is the player's own gear too.
        let Ok(Value::Object(props)) = serde_json::from_str::<Value>(properties) else {
            return;
        };
        for (field, component) in &props {
            let category = match field.as_str() {
                "weapon_entity" => continue,
                "amp_entity" => CostCategory::Amplifier,
                "implant_entity" => CostCategory::MindforceImplant,
                other if other.ends_with("_entity") => CostCategory::Attachment,
                _ => continue,
            };
            if let Some(component_name) = entity_name(component) {
                self.library.entry(component_name).or_insert(category);
            }
        }
    }
}

/// The digest of role inputs: the library rows, the sets and the game
/// data's categories, each in a fixed order.
fn fingerprint(
    library: &[(String, String, String)],
    sets: &[(String, String, String)],
    pieces: &[PieceRow],
    placements: &[(String, CostCategory)],
    catalogue: &HashMap<String, CostCategory>,
) -> String {
    let mut hasher = Sha256::new();
    let mut feed = |section: &str, mut rows: Vec<String>| {
        rows.sort_unstable();
        hasher.update(section.as_bytes());
        for row in rows {
            hasher.update((row.len() as u64).to_le_bytes());
            hasher.update(row.as_bytes());
        }
    };
    let joined = |parts: [&str; 3]| parts.join("\u{1f}");
    feed(
        "library",
        library
            .iter()
            .map(|(name, kind, properties)| joined([name, kind, properties]))
            .collect(),
    );
    feed(
        "sets",
        sets.iter()
            .map(|(kind, name, economy)| joined([kind, name, economy]))
            .collect(),
    );
    feed(
        "pieces",
        pieces
            .iter()
            .map(|row| {
                format!(
                    "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{:?}",
                    row.item, row.set_id, row.markup_percent, row.confirmed_at, row.removed_at
                )
            })
            .collect(),
    );
    feed(
        "placements",
        placements
            .iter()
            .map(|(item, category)| joined([item, category.as_str(), ""]))
            .collect(),
    );
    feed(
        "catalogue",
        catalogue
            .iter()
            .map(|(name, category)| joined([name, category.as_str(), ""]))
            .collect(),
    );
    hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl RoleSource for KnownRoles {
    fn library_category(&self, name: &str) -> Option<CostCategory> {
        self.library.get(name.trim()).copied()
    }

    fn dose_pricing(&self, name: &str) -> Option<DosePricing> {
        self.doses.get(name.trim()).copied()
    }

    fn placed_category(&self, name: &str) -> Option<CostCategory> {
        self.placements.get(name.trim()).copied()
    }

    fn held_as_loot_or_stock(&self, name: &str) -> bool {
        self.held.contains(name.trim())
    }

    fn catalogue_category(&self, name: &str) -> Option<CostCategory> {
        self.catalogue.get(name.trim()).copied()
    }

    /// The longest family name the item's name is, or starts with followed
    /// by a space; none when two families of that length disagree.
    fn protection_category(&self, name: &str) -> Option<CostCategory> {
        let mut best: Option<(usize, Option<CostCategory>)> = None;
        for (family, category) in &self.sets {
            if !family_names(family, name) {
                continue;
            }
            best = match best {
                Some((length, _)) if family.len() < length => best,
                Some((length, Some(held))) if family.len() == length && held != *category => {
                    Some((length, None))
                }
                Some((length, None)) if family.len() == length => best,
                _ => Some((family.len(), Some(*category))),
            };
        }
        best.and_then(|(_, category)| category)
    }
}

/// Whether a protection set's family name names an item: the item's name
/// is the family, or starts with it followed by a space, ignoring case.
pub fn family_names(family: &str, name: &str) -> bool {
    let family = family.trim().to_lowercase();
    let name = name.trim().to_lowercase();
    !family.is_empty()
        && (name == family
            || name
                .strip_prefix(family.as_str())
                .is_some_and(|rest| rest.starts_with(' ')))
}

/// What the readings show about one name.
#[derive(Debug, Default)]
struct Facts {
    largest_row: i64,
    /// Whether any row of the name held TT.
    any_value: bool,
    wears: bool,
    /// Names of the items this name sits attached to.
    parents: BTreeSet<String>,
    /// Names of the items attached to this name.
    children: BTreeSet<String>,
}

/// The role and stacking of every name a run of readings holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoleBook {
    entries: BTreeMap<String, (Role, Stacking)>,
    /// The names the library or the game data know as a stack that is not
    /// used up by the unit (an enhancer, say): their leaving is never
    /// consumption.
    known_materials: BTreeSet<String>,
}

impl RoleBook {
    /// Resolve every name the readings hold. The readings are in time
    /// order; an item wears when its value falls between two consecutive
    /// readings while it stays one item per row.
    pub fn resolve(source: &dyn RoleSource, readings: &[Reading]) -> RoleBook {
        let mut facts: BTreeMap<String, Facts> = BTreeMap::new();
        for reading in readings {
            for (key, holding) in reading.holdings() {
                let fact = facts.entry(key.name.clone()).or_default();
                let largest = holding.rows().iter().map(|row| row.quantity).max();
                fact.largest_row = fact.largest_row.max(largest.unwrap_or(0));
                fact.any_value |= holding.rows().iter().any(|row| row.value.0 != 0);
                if let Container::Attached { parent } = &key.container {
                    fact.parents.insert(parent.clone());
                    facts
                        .entry(parent.clone())
                        .or_default()
                        .children
                        .insert(key.name.clone());
                }
            }
        }
        for pair in readings.windows(2) {
            for (key, before) in pair[0].holdings() {
                let Some(after) = pair[1].holding(key) else {
                    continue;
                };
                let single = before.rows().iter().all(|row| row.quantity <= 1);
                if single && after.quantity() == before.quantity() && after.value() < before.value()
                {
                    if let Some(fact) = facts.get_mut(&key.name) {
                        fact.wears = true;
                    }
                }
            }
        }

        let direct: BTreeMap<&str, Option<Role>> = facts
            .keys()
            .map(|name| (name.as_str(), direct_role(source, name)))
            .collect();
        let direct_of = |name: &str| direct.get(name).copied().flatten();
        let mut entries = BTreeMap::new();
        for (name, fact) in &facts {
            let role = direct_of(name).unwrap_or_else(|| {
                let parent_of_plate = fact
                    .children
                    .iter()
                    .any(|child| direct_of(child) == Some(Role::Plating));
                let on_weapon = fact
                    .parents
                    .iter()
                    .any(|parent| direct_of(parent) == Some(Role::Weapon));
                if parent_of_plate {
                    Role::Armour
                } else if on_weapon {
                    if names_an_amplifier(name) {
                        Role::Amplifier
                    } else {
                        Role::OtherEquipment
                    }
                } else if fact.largest_row <= 1 && fact.wears {
                    Role::OtherEquipment
                } else {
                    Role::Material
                }
            });
            // A name only ever held one to a row is a single item, unless it
            // is ammo or a consumable the player's library, the game data or
            // its name knows: clothes, prizes and tokens arrive and leave as
            // items.
            let known_consumable = direct_of(name) == Some(Role::Material);
            // Keys and zero-TT strongboxes are always counted in units, so
            // their leaving is always read as an opening's.
            let opening_input = name == STRONGBOX_KEY || (is_strongbox(name) && !fact.any_value);
            let stacking = if fact.largest_row > 1
                || role == Role::Ammo
                || known_consumable
                || opening_input
            {
                Stacking::Stack
            } else {
                Stacking::Single
            };
            entries.insert(name.clone(), (role, stacking));
        }
        let known_materials = facts
            .keys()
            .filter(|name| is_material(source, name))
            .cloned()
            .collect();
        RoleBook {
            entries,
            known_materials,
        }
    }

    /// Whether the library or the game data know a name the readings held
    /// as a stack that is not used up by the unit.
    pub fn is_known_material(&self, name: &str) -> bool {
        self.known_materials.contains(name)
    }

    /// The role of a name the readings held; a name they did not hold has
    /// no evidence, so it is a material.
    pub fn role(&self, name: &str) -> Role {
        self.entries
            .get(name)
            .map_or(Role::Material, |(role, _)| *role)
    }

    /// The stacking of a name the readings held; a name they did not hold
    /// has no evidence of being one item, so it counts units.
    pub fn stacking(&self, name: &str) -> Stacking {
        self.entries
            .get(name)
            .map_or(Stacking::Stack, |(_, stacking)| *stacking)
    }
}

/// Whether a name is a stack the unit never uses up, so its leaving is never
/// consumption: the library or the game data know it as a material (an
/// enhancer), or nothing names it, the player has not placed it, and the app
/// has recorded it as loot or holds it as stock (an animal hide traded away).
pub fn is_material(source: &dyn RoleSource, name: &str) -> bool {
    match known_category(source, name) {
        Some(category) => category.role() == Role::Material && !category.is_consumable(),
        None => source.placed_category(name).is_none() && source.held_as_loot_or_stock(name),
    }
}

/// The role a name has from evidence about the name alone.
fn direct_role(source: &dyn RoleSource, name: &str) -> Option<Role> {
    known_category(source, name).map(CostCategory::role)
}

/// The category a name has from evidence about the name alone: the
/// library, then the game data, then the name itself, then a protection
/// set (limited when the set is or the name's tag says so).
pub fn known_category(source: &dyn RoleSource, name: &str) -> Option<CostCategory> {
    source
        .library_category(name)
        .or_else(|| source.catalogue_category(name))
        .or_else(|| name_category(name))
        .or_else(|| {
            source
                .protection_category(name)
                .map(|category| category.limited_when(name_is_limited(name)))
        })
}

/// The category an item's own name gives it.
fn name_category(name: &str) -> Option<CostCategory> {
    if name == UNIVERSAL_AMMO {
        Some(CostCategory::Ammo)
    } else if name.starts_with(PLATING_PREFIX) {
        Some(CostCategory::UnlimitedPlate.limited_when(name_is_limited(name)))
    } else if name == MIND_ESSENCE {
        Some(CostCategory::MindEssence)
    } else if name.starts_with(TELEPORT_CHIP_PREFIX) {
        Some(CostCategory::TeleportChip)
    } else if FIELD_LOGISTICS_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        Some(CostCategory::FieldLogistics)
    } else {
        None
    }
}

/// The category one measured use of an item costs as. The player's own
/// placement of the item answers first; then the item's own category, when
/// anything names it; otherwise it is read from the
/// role the measure gave it, which carries how the readings held it (the
/// parent of a plate is armour, an item on a weapon an amplifier or an
/// attachment), with `parent_role` the role of the item it sits attached
/// to. A healing item used up by the unit is a healing consumable however
/// it is named; armour and plates are limited when their name's tag says
/// so. What nothing names and no structure places is unclassified.
pub fn cost_category(
    source: &dyn RoleSource,
    key: &HoldingKey,
    role: Role,
    used: Use,
    parent_role: Option<Role>,
) -> CostCategory {
    let limited = name_is_limited(&key.name);
    let attached = matches!(key.container, Container::Attached { .. });
    let known = source
        .placed_category(&key.name)
        .or_else(|| known_category(source, &key.name));
    let category = known.unwrap_or(match role {
        Role::Ammo => CostCategory::Ammo,
        Role::Weapon => CostCategory::Weapon,
        Role::Amplifier => CostCategory::Amplifier,
        Role::Healing => CostCategory::HealingTool,
        Role::Armour => CostCategory::UnlimitedArmour.limited_when(limited),
        Role::Plating => CostCategory::UnlimitedPlate.limited_when(limited),
        Role::OtherEquipment if attached && parent_role == Some(Role::Weapon) => {
            CostCategory::Attachment
        }
        Role::OtherEquipment | Role::Material => CostCategory::Unclassified,
    });
    match (category, used) {
        (CostCategory::HealingTool, Use::Units) => CostCategory::HealingConsumable,
        (category, _) => category,
    }
}

/// Whether a name calls itself an amplifier (`Laser Amp`, `MF-Amplifier`).
fn names_an_amplifier(name: &str) -> bool {
    name.split(|c: char| !c.is_alphanumeric())
        .any(|word| word.eq_ignore_ascii_case("amp") || word.eq_ignore_ascii_case("amplifier"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::amount::Pec;
    use crate::account::reading::{Holding, HoldingKey, Row};

    /// A stand-in for the player's data and the game data.
    #[derive(Default)]
    struct Stand {
        library: Vec<(&'static str, CostCategory)>,
        catalogue: Vec<(&'static str, CostCategory)>,
        protection: Vec<(&'static str, CostCategory)>,
    }

    impl RoleSource for Stand {
        fn library_category(&self, name: &str) -> Option<CostCategory> {
            self.library
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, c)| *c)
        }
        fn catalogue_category(&self, name: &str) -> Option<CostCategory> {
            self.catalogue
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, c)| *c)
        }
        fn protection_category(&self, name: &str) -> Option<CostCategory> {
            self.protection
                .iter()
                .find(|(n, _)| name.starts_with(n))
                .map(|(_, c)| *c)
        }
    }

    fn reading(at: f64, rows: &[(&str, Container, i64, i64)]) -> Reading {
        let mut holdings: BTreeMap<HoldingKey, Vec<Row>> = BTreeMap::new();
        for (name, container, quantity, value) in rows {
            holdings
                .entry(HoldingKey {
                    name: name.to_string(),
                    container: container.clone(),
                })
                .or_default()
                .push(Row {
                    quantity: *quantity,
                    value: Pec(*value),
                });
        }
        Reading::new(
            at,
            Pec(0),
            holdings
                .into_iter()
                .map(|(key, rows)| (key, Holding::new(rows).unwrap()))
                .collect(),
        )
        .unwrap()
    }

    fn attached(parent: &str) -> Container {
        Container::Attached {
            parent: parent.to_string(),
        }
    }

    #[test]
    fn the_library_answers_before_the_game_data_and_the_name() {
        let stand = Stand {
            library: vec![("Universal Ammo", CostCategory::Consumable)],
            catalogue: vec![
                ("Universal Ammo", CostCategory::Weapon),
                ("Opalo", CostCategory::Weapon),
            ],
            ..Stand::default()
        };
        let book = RoleBook::resolve(
            &stand,
            &[reading(
                0.0,
                &[
                    ("Universal Ammo", Container::Carried, 10, 0),
                    ("Opalo", Container::Carried, 1, 100),
                ],
            )],
        );
        assert_eq!(book.role("Universal Ammo"), Role::Material);
        assert_eq!(book.role("Opalo"), Role::Weapon);
        assert_eq!(book.stacking("Opalo"), Stacking::Single);
    }

    #[test]
    fn names_carry_ammo_and_plating_and_a_plate_s_parent_is_armour() {
        let book = RoleBook::resolve(
            &Stand::default(),
            &[reading(
                0.0,
                &[
                    ("Universal Ammo", Container::Carried, 9, 0),
                    (
                        "Armor Plating Mk. 6A",
                        attached("Paneleon Arm Guards (M)"),
                        1,
                        2500,
                    ),
                    ("Paneleon Arm Guards (M)", Container::Carried, 1, 2150),
                ],
            )],
        );
        assert_eq!(book.role("Universal Ammo"), Role::Ammo);
        assert_eq!(book.stacking("Universal Ammo"), Stacking::Stack);
        assert_eq!(book.role("Armor Plating Mk. 6A"), Role::Plating);
        assert_eq!(book.role("Paneleon Arm Guards (M)"), Role::Armour);
        assert_eq!(book.stacking("Paneleon Arm Guards (M)"), Stacking::Single);
    }

    #[test]
    fn an_item_on_a_weapon_is_its_amplifier_or_an_attachment() {
        let stand = Stand {
            catalogue: vec![("Chip 15", CostCategory::Weapon)],
            ..Stand::default()
        };
        let book = RoleBook::resolve(
            &stand,
            &[reading(
                0.0,
                &[
                    ("Chip 15", Container::Carried, 1, 100),
                    ("Mayhem MF-Amplifier Delta (L)", attached("Chip 15"), 1, 846),
                    ("Odd Scope", attached("Chip 15"), 1, 50),
                ],
            )],
        );
        assert_eq!(book.role("Mayhem MF-Amplifier Delta (L)"), Role::Amplifier);
        assert_eq!(book.role("Odd Scope"), Role::OtherEquipment);
    }

    #[test]
    fn exact_names_answer_before_a_protection_family() {
        let stand = Stand {
            catalogue: vec![("Frontier Combat Knife", CostCategory::Weapon)],
            protection: vec![("Frontier", CostCategory::UnlimitedArmour)],
            ..Stand::default()
        };
        let book = RoleBook::resolve(
            &stand,
            &[reading(
                0.0,
                &[
                    ("Frontier Combat Knife", Container::Carried, 1, 200),
                    ("Frontier Gloves, Adjusted (M)", Container::Carried, 1, 200),
                ],
            )],
        );
        assert_eq!(book.role("Frontier Combat Knife"), Role::Weapon);
        assert_eq!(book.role("Frontier Gloves, Adjusted (M)"), Role::Armour);
    }

    #[test]
    fn a_single_item_that_wears_is_equipment_and_anything_else_a_material() {
        let readings = [
            reading(
                0.0,
                &[
                    ("Teleportation Chip 1", Container::Carried, 1, 8665),
                    ("Christmas Coat 2025 (M)", Container::Carried, 1, 100),
                    ("Mayhem Token", Container::Carried, 1, 0),
                    ("Shrapnel", Container::Carried, 81368, 813),
                ],
            ),
            reading(
                60.0,
                &[
                    ("Teleportation Chip 1", Container::Carried, 1, 8663),
                    ("Christmas Coat 2025 (M)", Container::Carried, 1, 100),
                    ("Mayhem Token", Container::Carried, 1, 0),
                    ("Shrapnel", Container::Carried, 81000, 810),
                ],
            ),
        ];
        let book = RoleBook::resolve(&Stand::default(), &readings);
        assert_eq!(book.role("Teleportation Chip 1"), Role::OtherEquipment);
        assert_eq!(book.stacking("Teleportation Chip 1"), Stacking::Single);
        assert_eq!(book.role("Christmas Coat 2025 (M)"), Role::Material);
        assert_eq!(
            book.stacking("Mayhem Token"),
            Stacking::Single,
            "a token only ever held one to a row arrives and leaves as an item"
        );
        assert_eq!(book.stacking("Christmas Coat 2025 (M)"), Stacking::Single);
        assert_eq!(book.stacking("Shrapnel"), Stacking::Stack);
        assert_eq!(book.role("Shrapnel"), Role::Material);
        assert_eq!(book.role("never held"), Role::Material);
    }

    #[test]
    fn a_name_seen_in_rows_of_more_than_one_counts_units_whatever_its_role() {
        let stand = Stand {
            catalogue: vec![("Odd Weapon", CostCategory::Weapon)],
            ..Stand::default()
        };
        let book = RoleBook::resolve(
            &stand,
            &[reading(0.0, &[("Odd Weapon", Container::Carried, 3, 30)])],
        );
        assert_eq!(book.role("Odd Weapon"), Role::Weapon);
        assert_eq!(book.stacking("Odd Weapon"), Stacking::Stack);
    }

    #[test]
    fn keys_and_zero_tt_strongboxes_held_singly_still_count_units() {
        let book = RoleBook::resolve(
            &Stand::default(),
            &[reading(
                0.0,
                &[
                    ("Strongbox Key", Container::Carried, 1, 1_000),
                    ("Halloween Strongbox", Container::Carried, 1, 0),
                    ("Golden Strongbox", Container::Carried, 1, 500),
                ],
            )],
        );
        assert_eq!(book.stacking("Strongbox Key"), Stacking::Stack);
        assert_eq!(book.stacking("Halloween Strongbox"), Stacking::Stack);
        assert_eq!(
            book.stacking("Golden Strongbox"),
            Stacking::Single,
            "a strongbox with TT is no opening's box"
        );
    }

    #[test]
    fn a_known_consumable_held_one_at_a_time_still_counts_units() {
        let stand = Stand {
            library: vec![("Loot Collection Pill", CostCategory::Consumable)],
            catalogue: vec![("Vivo T1", CostCategory::Consumable)],
            ..Stand::default()
        };
        let book = RoleBook::resolve(
            &stand,
            &[reading(
                0.0,
                &[
                    ("Loot Collection Pill", Container::Carried, 1, 10),
                    ("Vivo T1", Container::Carried, 1, 5),
                    ("Skeleton Pants Red (F)", Container::Carried, 1, 1000),
                ],
            )],
        );
        assert_eq!(book.stacking("Loot Collection Pill"), Stacking::Stack);
        assert_eq!(book.stacking("Vivo T1"), Stacking::Stack);
        assert_eq!(book.stacking("Skeleton Pants Red (F)"), Stacking::Single);
    }

    #[test]
    fn known_roles_read_the_library_its_weapon_parts_the_sets_and_the_game_data() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("weapons.json"),
            r#"[{"id": 1, "name": "Frontier Combat Knife"}]"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("medical_tools.json"),
            r#"[{"id": 2, "name": "Frontier Combat Knife"}, {"id": 3, "name": " Vivo T1 "}]"#,
        )
        .unwrap();
        let store = GameDataStore::new(dir.path()).unwrap();
        let library = vec![
            (
                "Chip 15".to_string(),
                "weapon".to_string(),
                r#"{"weapon_entity": {"name": "Chip 15"}, "amp_entity": {"name": "Delta Amp"},
                    "implant_entity": {"name": "NeoPsion 85-B"}, "scope_entity": null}"#
                    .to_string(),
            ),
            ("FAP".to_string(), "healing".to_string(), "{}".to_string()),
            (
                "Pill".to_string(),
                "consumable".to_string(),
                "{}".to_string(),
            ),
            (
                "Mystery".to_string(),
                "gadget".to_string(),
                "{}".to_string(),
            ),
        ];
        let sets = vec![
            (
                "armour".to_string(),
                "Pegasus".to_string(),
                "limited".to_string(),
            ),
            (
                "plates".to_string(),
                "6B".to_string(),
                "unlimited".to_string(),
            ),
            (
                "armour".to_string(),
                "Pegasus Elite".to_string(),
                "unlimited".to_string(),
            ),
        ];
        let known = KnownRoles::from_parts(&library, &sets, Some(&store));
        assert_eq!(known.library_role("Chip 15"), Some(Role::Weapon));
        assert_eq!(known.library_role("Delta Amp"), Some(Role::Amplifier));
        assert_eq!(
            known.library_role("NeoPsion 85-B"),
            Some(Role::OtherEquipment)
        );
        assert_eq!(known.library_role("FAP"), Some(Role::Healing));
        assert_eq!(known.library_role("Pill"), Some(Role::Material));
        assert_eq!(known.library_role("Mystery"), None);
        assert_eq!(
            known.catalogue_role("Frontier Combat Knife"),
            Some(Role::Weapon),
            "the weapon collection settles a name two collections hold"
        );
        assert_eq!(known.catalogue_role("Vivo T1"), Some(Role::Healing));
        assert_eq!(
            known.protection_role("Pegasus Helmet (M,L)"),
            Some(Role::Armour)
        );
        assert_eq!(known.protection_role("pegasus"), Some(Role::Armour));
        assert_eq!(known.protection_role("Pegasusx Helmet"), None);
        assert_eq!(known.protection_role("6B Plate"), Some(Role::Plating));
        assert_eq!(known.protection_role("Hyperion Helmet"), None);
        // The finer categories the roles are read from.
        assert_eq!(
            known.library_category("NeoPsion 85-B"),
            Some(CostCategory::MindforceImplant)
        );
        assert_eq!(
            known.library_category("Delta Amp"),
            Some(CostCategory::Amplifier)
        );
        assert_eq!(
            known.library_category("FAP"),
            Some(CostCategory::HealingTool)
        );
        assert_eq!(
            known.library_category("Pill"),
            Some(CostCategory::Consumable)
        );
        assert_eq!(
            known.protection_category("Pegasus Helmet (M,L)"),
            Some(CostCategory::LimitedArmour),
            "a limited set's pieces are limited"
        );
        assert_eq!(
            known.protection_category("Pegasus Elite Helmet"),
            Some(CostCategory::UnlimitedArmour)
        );
        assert_eq!(
            known.protection_category("6B Plate"),
            Some(CostCategory::UnlimitedPlate)
        );
    }

    #[test]
    fn the_game_data_s_collections_name_their_categories() {
        let dir = tempfile::tempdir().unwrap();
        for (file, name) in [
            ("weapon_vision_attachments.json", "Odd Scope"),
            ("absorbers.json", "Odd Extender P10 (L)"),
            ("mindforce_implants.json", "Odd Implant"),
            ("harvesting_tools.json", "Odd Feller"),
            ("stimulants.json", "Odd Stim"),
            ("enhancers.json", "Odd Enhancer"),
        ] {
            std::fs::write(
                dir.path().join(file),
                format!(r#"[{{"id": 1, "name": "{name}"}}]"#),
            )
            .unwrap();
        }
        let store = GameDataStore::new(dir.path()).unwrap();
        let known = KnownRoles::from_parts(&[], &[], Some(&store));
        for (name, category) in [
            ("Odd Scope", CostCategory::Attachment),
            ("Odd Extender P10 (L)", CostCategory::Attachment),
            ("Odd Implant", CostCategory::MindforceImplant),
            ("Odd Feller", CostCategory::HarvestingTool),
            ("Odd Stim", CostCategory::Consumable),
            ("Odd Enhancer", CostCategory::Enhancer),
        ] {
            assert_eq!(known.catalogue_category(name), Some(category), "{name}");
        }
    }

    #[test]
    fn a_name_s_closing_tag_says_whether_it_is_limited() {
        for limited in [
            "Pegasus Helmet (M,L)",
            "Armor Plating Mk. 5B (L)",
            "California FAP mk III (C, L)",
            " Odd Gloves (F,L) ",
        ] {
            assert!(name_is_limited(limited), "{limited}");
        }
        for unlimited in [
            "Paneleon Spec Imperium Arm Guards (M)",
            "Armor Plating Mk. 6A",
            "Lucky (L) Charm",
            "Odd (LL)",
            "Odd (L",
        ] {
            assert!(!name_is_limited(unlimited), "{unlimited}");
        }
    }

    fn key(name: &str, container: Container) -> HoldingKey {
        HoldingKey {
            name: name.to_string(),
            container,
        }
    }

    #[test]
    fn a_measured_use_costs_as_its_known_category_or_as_its_place_shows() {
        let stand = Stand {
            library: vec![("Opalo", CostCategory::Weapon)],
            catalogue: vec![("Vivo T1", CostCategory::HealingTool)],
            protection: vec![
                ("Pegasus", CostCategory::LimitedArmour),
                ("Frontier", CostCategory::UnlimitedArmour),
            ],
        };
        let carried = |name: &str| key(name, Container::Carried);
        let cost = |key: &HoldingKey, role: Role, used: Use, parent: Option<Role>| {
            cost_category(&stand, key, role, used, parent)
        };
        // Named by the library, the game data, the name or a set.
        assert_eq!(
            cost(&carried("Opalo"), Role::Weapon, Use::Wear, None),
            CostCategory::Weapon
        );
        assert_eq!(
            cost(
                &carried("Teleportation Chip 1"),
                Role::OtherEquipment,
                Use::Wear,
                None
            ),
            CostCategory::TeleportChip
        );
        assert_eq!(
            cost(&carried("Mind Essence"), Role::Material, Use::Units, None),
            CostCategory::MindEssence
        );
        for tool in ["Portable Repair Unit (L)", "Portable TT Unit"] {
            assert_eq!(
                cost(&carried(tool), Role::OtherEquipment, Use::Wear, None),
                CostCategory::FieldLogistics,
                "{tool}"
            );
        }
        assert_eq!(
            cost(
                &carried("Pegasus Helmet (M,L)"),
                Role::Armour,
                Use::Wear,
                None
            ),
            CostCategory::LimitedArmour
        );
        assert_eq!(
            cost(
                &carried("Frontier Gloves, Adjusted (M)"),
                Role::Armour,
                Use::Wear,
                None
            ),
            CostCategory::UnlimitedArmour
        );
        assert_eq!(
            cost(
                &carried("Frontier Gloves (L)"),
                Role::Armour,
                Use::Wear,
                None
            ),
            CostCategory::LimitedArmour,
            "a piece whose name says limited is, whatever its set"
        );
        let plate = key(
            "Armor Plating Mk. 5B (L)",
            Container::Attached {
                parent: "Odd Harness (M)".to_string(),
            },
        );
        assert_eq!(
            cost(&plate, Role::Plating, Use::Wear, Some(Role::Armour)),
            CostCategory::LimitedPlate
        );
        // A healing tool's units used up are a healing consumable.
        assert_eq!(
            cost(&carried("Vivo T1"), Role::Healing, Use::Wear, None),
            CostCategory::HealingTool
        );
        assert_eq!(
            cost(&carried("Vivo T1"), Role::Healing, Use::Units, None),
            CostCategory::HealingConsumable
        );
        // Placed by how the readings held it.
        assert_eq!(
            cost(&carried("Odd Harness (M)"), Role::Armour, Use::Wear, None),
            CostCategory::UnlimitedArmour
        );
        let on_weapon = key(
            "Odd Sight",
            Container::Attached {
                parent: "Opalo".to_string(),
            },
        );
        assert_eq!(
            cost(
                &on_weapon,
                Role::OtherEquipment,
                Use::Wear,
                Some(Role::Weapon)
            ),
            CostCategory::Attachment
        );
        assert_eq!(
            cost(
                &on_weapon,
                Role::OtherEquipment,
                Use::Wear,
                Some(Role::Armour)
            ),
            CostCategory::Unclassified,
            "an item on anything but a weapon is no attachment"
        );
        // Nothing names it and nothing places it.
        assert_eq!(
            cost(
                &carried("Exploration Hoverpod MkI (L)"),
                Role::OtherEquipment,
                Use::Wear,
                None
            ),
            CostCategory::Unclassified
        );
        assert_eq!(
            cost(
                &carried("Animal Oil Residue"),
                Role::Material,
                Use::Units,
                None
            ),
            CostCategory::Unclassified
        );
    }

    #[test]
    fn every_category_reads_back_from_its_stored_name() {
        for category in CostCategory::ALL {
            assert_eq!(CostCategory::parse(category.as_str()), Some(category));
        }
        assert_eq!(CostCategory::parse("later"), None);
    }

    #[test]
    fn mind_essence_held_one_unit_at_a_time_still_counts_units() {
        let book = RoleBook::resolve(
            &Stand::default(),
            &[reading(0.0, &[("Mind Essence", Container::Carried, 1, 0)])],
        );
        assert_eq!(book.stacking("Mind Essence"), Stacking::Stack);
        assert_eq!(book.role("Mind Essence"), Role::Material);
    }

    #[test]
    fn two_equally_long_families_that_disagree_give_no_role() {
        let sets = vec![
            (
                "armour".to_string(),
                "Ghost".to_string(),
                "unlimited".to_string(),
            ),
            (
                "plates".to_string(),
                "ghost".to_string(),
                "unlimited".to_string(),
            ),
        ];
        let known = KnownRoles::from_parts(&[], &sets, None);
        assert_eq!(known.protection_role("Ghost Harness"), None);
    }

    #[tokio::test]
    async fn known_roles_read_the_player_s_tables_without_writing_them() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(&dir.path().join("roles.db"))
            .await
            .unwrap();
        db.insert_equipment_for_tests(
            1,
            "Chip 15",
            "weapon",
            r#"{"weapon_entity": {"name": "Chip 15"}, "amp_entity": {"name": "Delta Amp"}}"#,
        )
        .await
        .unwrap();
        let known = db
            .with_writer(|conn| {
                conn.execute(
                    "INSERT INTO protection_sets (kind, name, economy_kind, markup_percent, \
                     created_at) VALUES ('armour', 'Pegasus', 'limited', 120, 1)",
                    [],
                )?;
                let changes_before = conn.total_changes();
                let known = KnownRoles::read(conn, None).map_err(|error| match error {
                    AccountError::Db(inner) => inner,
                    other => panic!("{other}"),
                })?;
                assert_eq!(
                    conn.total_changes(),
                    changes_before,
                    "reading writes nothing"
                );
                Ok(known)
            })
            .await
            .unwrap();
        assert_eq!(known.library_role("Delta Amp"), Some(Role::Amplifier));
        assert_eq!(
            known.protection_role("Pegasus Gloves (M,L)"),
            Some(Role::Armour)
        );
    }
}
