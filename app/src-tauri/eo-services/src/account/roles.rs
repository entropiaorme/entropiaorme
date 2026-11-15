//! What an item in a reading is, for grouping the measure.
//!
//! A role only groups: every measured figure is also kept per item, so a
//! wrong role shows as an item in the wrong group rather than vanishing.
//! A name's role is the first answer from, in order:
//!
//! 1. the player's equipment library (an entry's own name, and the
//!    amplifier and attachments a weapon entry carries);
//! 2. the bundled game data (the weapon, amplifier, medical tool,
//!    attachment, implant, harvesting tool, stimulant and enhancer
//!    collections);
//! 3. the item's own name: Universal Ammo is ammo, a name starting
//!    `Armor Plating` is plating;
//! 4. the player's protection sets, whose names name a family of pieces
//!    (`Pegasus` names `Pegasus Helmet (M,L)`);
//! 5. how the reading holds it: the parent of an attached plate is armour,
//!    and an item attached to a weapon is its amplifier or an attachment;
//! 6. otherwise, a single item that wears is other equipment, and anything
//!    else is a material.
//!
//! Exact names (the library and the game data) answer before the
//! protection sets' family names, so a set named after a maker cannot
//! claim that maker's weapons. Resolution only reads: nothing here writes
//! the library, the sets, or the game data.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::reading::{Container, Reading};
use super::AccountError;
use crate::bundled_tools::entity_name;
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

/// The game-data collections that name a role, in the order a name found
/// in two of them is settled.
const CATALOGUE_ROLES: &[(&str, Role)] = &[
    ("weapons", Role::Weapon),
    ("weapon_amplifiers", Role::Amplifier),
    ("medical_tools", Role::Healing),
    ("weapon_vision_attachments", Role::OtherEquipment),
    ("absorbers", Role::OtherEquipment),
    ("mindforce_implants", Role::OtherEquipment),
    ("harvesting_tools", Role::OtherEquipment),
    ("stimulants", Role::Material),
    ("enhancers", Role::Material),
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

/// Where role evidence comes from, so tests can stand one in.
pub trait RoleSource {
    /// The role the player's equipment library gives this exact name.
    fn library_role(&self, name: &str) -> Option<Role>;
    /// The role the bundled game data gives this exact name.
    fn catalogue_role(&self, name: &str) -> Option<Role>;
    /// The role a protection set's family name gives this name.
    fn protection_role(&self, name: &str) -> Option<Role>;
}

/// The player's library and sets and the bundled game data, read once.
#[derive(Debug, Clone, Default)]
pub struct KnownRoles {
    library: HashMap<String, Role>,
    catalogue: HashMap<String, Role>,
    /// Lower-cased family name and the role its pieces take.
    sets: Vec<(String, Role)>,
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
            let mut stmt = conn.prepare("SELECT kind, name FROM protection_sets")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(KnownRoles::from_parts(&library, &sets, game_data))
    }

    /// Build from library rows `(name, item type, properties)`, protection
    /// sets `(kind, name)`, and the game data.
    pub fn from_parts(
        library: &[(String, String, String)],
        sets: &[(String, String)],
        game_data: Option<&GameDataStore>,
    ) -> KnownRoles {
        let mut known = KnownRoles::default();
        for (name, item_type, properties) in library {
            known.add_library_entry(name, item_type, properties);
        }
        for (kind, name) in sets {
            let role = if kind == ProtectionSetKind::Armour.as_str() {
                Role::Armour
            } else if kind == ProtectionSetKind::Plates.as_str() {
                Role::Plating
            } else {
                continue;
            };
            let family = name.trim().to_lowercase();
            if !family.is_empty() {
                known.sets.push((family, role));
            }
        }
        if let Some(store) = game_data {
            for (endpoint, role) in CATALOGUE_ROLES {
                for entity in store.get_entities(endpoint) {
                    if let Some(name) = entity_name(entity) {
                        known.catalogue.entry(name).or_insert(*role);
                    }
                }
            }
        }
        known
    }

    fn add_library_entry(&mut self, name: &str, item_type: &str, properties: &str) {
        let own = match item_type {
            "weapon" => Role::Weapon,
            "healing" => Role::Healing,
            "tool" => Role::OtherEquipment,
            "consumable" => Role::Material,
            _ => return,
        };
        self.library.entry(name.trim().to_string()).or_insert(own);
        if own != Role::Weapon {
            return;
        }
        // A weapon entry names the amplifier and attachments it is used
        // with; each is the player's own gear too.
        let Ok(Value::Object(props)) = serde_json::from_str::<Value>(properties) else {
            return;
        };
        for (field, component) in &props {
            let role = match field.as_str() {
                "weapon_entity" => continue,
                "amp_entity" => Role::Amplifier,
                other if other.ends_with("_entity") => Role::OtherEquipment,
                _ => continue,
            };
            if let Some(component_name) = entity_name(component) {
                self.library.entry(component_name).or_insert(role);
            }
        }
    }
}

impl RoleSource for KnownRoles {
    fn library_role(&self, name: &str) -> Option<Role> {
        self.library.get(name.trim()).copied()
    }

    fn catalogue_role(&self, name: &str) -> Option<Role> {
        self.catalogue.get(name.trim()).copied()
    }

    /// The longest family name the item's name is, or starts with followed
    /// by a space; none when two families of that length disagree.
    fn protection_role(&self, name: &str) -> Option<Role> {
        let name = name.trim().to_lowercase();
        let mut best: Option<(usize, Option<Role>)> = None;
        for (family, role) in &self.sets {
            let matches = name == *family
                || name
                    .strip_prefix(family.as_str())
                    .is_some_and(|rest| rest.starts_with(' '));
            if !matches {
                continue;
            }
            best = match best {
                Some((length, _)) if family.len() < length => best,
                Some((length, Some(held))) if family.len() == length && held != *role => {
                    Some((length, None))
                }
                Some((length, None)) if family.len() == length => best,
                _ => Some((family.len(), Some(*role))),
            };
        }
        best.and_then(|(_, role)| role)
    }
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
            // is ammo or a consumable the player's library or the game data
            // knows: clothes, prizes and tokens arrive and leave as items.
            let known_consumable = source.library_role(name) == Some(Role::Material)
                || (source.library_role(name).is_none()
                    && source.catalogue_role(name) == Some(Role::Material));
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
        RoleBook { entries }
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

/// The role a name has from evidence about the name alone.
fn direct_role(source: &dyn RoleSource, name: &str) -> Option<Role> {
    source
        .library_role(name)
        .or_else(|| source.catalogue_role(name))
        .or_else(|| {
            if name == UNIVERSAL_AMMO {
                Some(Role::Ammo)
            } else if name.starts_with(PLATING_PREFIX) {
                Some(Role::Plating)
            } else {
                None
            }
        })
        .or_else(|| source.protection_role(name))
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
        library: Vec<(&'static str, Role)>,
        catalogue: Vec<(&'static str, Role)>,
        protection: Vec<(&'static str, Role)>,
    }

    impl RoleSource for Stand {
        fn library_role(&self, name: &str) -> Option<Role> {
            self.library
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, r)| *r)
        }
        fn catalogue_role(&self, name: &str) -> Option<Role> {
            self.catalogue
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, r)| *r)
        }
        fn protection_role(&self, name: &str) -> Option<Role> {
            self.protection
                .iter()
                .find(|(n, _)| name.starts_with(n))
                .map(|(_, r)| *r)
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
            library: vec![("Universal Ammo", Role::Material)],
            catalogue: vec![("Universal Ammo", Role::Weapon), ("Opalo", Role::Weapon)],
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
            catalogue: vec![("Chip 15", Role::Weapon)],
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
            catalogue: vec![("Frontier Combat Knife", Role::Weapon)],
            protection: vec![("Frontier", Role::Armour)],
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
            catalogue: vec![("Odd Weapon", Role::Weapon)],
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
            library: vec![("Loot Collection Pill", Role::Material)],
            catalogue: vec![("Vivo T1", Role::Material)],
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
            ("armour".to_string(), "Pegasus".to_string()),
            ("plates".to_string(), "6B".to_string()),
            ("armour".to_string(), "Pegasus Elite".to_string()),
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
    }

    #[test]
    fn two_equally_long_families_that_disagree_give_no_role() {
        let sets = vec![
            ("armour".to_string(), "Ghost".to_string()),
            ("plates".to_string(), "ghost".to_string()),
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
