//! The curated quest catalogue: the quests the app ships and maintains.
//!
//! The catalogue is reviewed data compiled into the binary
//! (`catalogue.json`), shaped the way a player thinks about it: a planet,
//! then a category, then the quests in it. A family entry is one repeatable
//! slot whose variants the giver rotates; a quest entry stands alone.
//!
//! Completions, runs and intervals all reference a quest row, so the
//! catalogue cannot live in memory alone: [`sync_catalogue`] materialises
//! it into the database at startup. A row is found by its catalogue key,
//! or adopted by name the first time (an alias covers a rename the
//! catalogue makes), so a quest the player already tracked keeps its whole
//! history. That adoption is also how a user-authored quest is promoted:
//! adding it to the catalogue claims the existing row.
//!
//! The sync owns only the fields the catalogue states. It never deletes a
//! row, never touches recorded play, and leaves notes and target mobs
//! alone. A keyed row the catalogue no longer carries is retired (soft
//! deleted), never removed.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::clock::Clock;
use crate::db::{Db, DbError};

use super::missions::normalize_quest_name;

/// The catalogue as compiled into this binary.
pub static CATALOGUE: LazyLock<Catalogue> = LazyLock::new(|| {
    serde_json::from_str(include_str!("catalogue.json")).expect("the quest catalogue parses")
});

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalogue {
    pub version: u32,
    pub planets: Vec<CataloguePlanet>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CataloguePlanet {
    pub key: String,
    /// The planet as quest rows store it.
    pub name: String,
    pub categories: Vec<CatalogueCategory>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueCategory {
    pub key: String,
    pub name: String,
    pub entries: Vec<CatalogueEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CatalogueEntry {
    Family(CatalogueFamily),
    Quest(CatalogueQuest),
}

impl CatalogueEntry {
    pub fn key(&self) -> &str {
        match self {
            CatalogueEntry::Family(family) => &family.key,
            CatalogueEntry::Quest(quest) => &quest.key,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueFamily {
    pub key: String,
    /// The family part of every member's mission name.
    pub name: String,
    /// What the board calls the slot.
    pub label: String,
    pub cooldown_hours: f64,
    pub cooldown_anchor: Anchor,
    pub reward: CatalogueReward,
    pub members: Vec<CatalogueMember>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueMember {
    pub key: String,
    /// The exact mission name ("Family: Variant").
    pub name: String,
    #[serde(default)]
    pub waypoint: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueQuest {
    pub key: String,
    /// The exact mission name.
    pub name: String,
    /// What the board calls it, when shorter than the mission name.
    #[serde(default)]
    pub label: Option<String>,
    /// A short qualifier the board shows beside the label.
    #[serde(default)]
    pub detail: Option<String>,
    pub completion: Completion,
    #[serde(default)]
    pub signal_item: Option<String>,
    pub cooldown_hours: f64,
    pub cooldown_anchor: Anchor,
    pub reward: CatalogueReward,
    #[serde(default)]
    pub waypoint: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    Pickup,
    Completion,
}

impl Anchor {
    pub fn as_str(self) -> &'static str {
        match self {
            Anchor::Pickup => "pickup",
            Anchor::Completion => "completion",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    MissionLog,
    SignalItem,
}

impl Completion {
    pub fn as_str(self) -> &'static str {
        match self {
            Completion::MissionLog => "mission_log",
            Completion::SignalItem => "signal_item",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueReward {
    pub policy: RewardPolicy,
    /// The named reward items, for `named_items`.
    #[serde(default)]
    pub items: Vec<String>,
    /// The fixed PES amount, for `fixed_pes`.
    #[serde(default)]
    pub amount: Option<f64>,
    /// How the board states the reward.
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewardPolicy {
    None,
    FixedPes,
    NamedItems,
    CompletionClump,
}

impl RewardPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            RewardPolicy::None => "none",
            RewardPolicy::FixedPes => "fixed_pes",
            RewardPolicy::NamedItems => "named_items",
            RewardPolicy::CompletionClump => "completion_clump",
        }
    }
}

impl Catalogue {
    /// Every structural rule the sync and the board rely on. The shipped
    /// catalogue is checked by a test; a malformed edit fails the build's
    /// tests rather than reaching a player's database.
    pub fn validate(&self) -> Result<(), String> {
        let mut keys = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut claim_key = |key: &str| -> Result<(), String> {
            if key.trim().is_empty() || !keys.insert(key.to_string()) {
                return Err(format!("catalogue key '{key}' is blank or repeated"));
            }
            Ok(())
        };
        for planet in &self.planets {
            claim_key(&planet.key)?;
            if planet.name.trim().is_empty() || planet.categories.is_empty() {
                return Err(format!("planet '{}' needs a name and a category", planet.key));
            }
            for category in &planet.categories {
                claim_key(&category.key)?;
                if category.entries.is_empty() {
                    return Err(format!("category '{}' is empty", category.key));
                }
                for entry in &category.entries {
                    claim_key(entry.key())?;
                    match entry {
                        CatalogueEntry::Family(family) => {
                            validate_reward(&family.key, &family.reward)?;
                            if family.members.is_empty() || family.cooldown_hours <= 0.0 {
                                return Err(format!(
                                    "family '{}' needs members and a cooldown",
                                    family.key
                                ));
                            }
                            if !names.insert(normalize_quest_name(&family.name)) {
                                return Err(format!("family name '{}' is repeated", family.name));
                            }
                            for member in &family.members {
                                claim_key(&member.key)?;
                                let Some((part, variant)) = member.name.rsplit_once(':') else {
                                    return Err(format!(
                                        "member '{}' is not named 'Family: Variant'",
                                        member.key
                                    ));
                                };
                                if normalize_quest_name(part) != normalize_quest_name(&family.name)
                                    || variant.trim().is_empty()
                                {
                                    return Err(format!(
                                        "member '{}' does not belong to '{}'",
                                        member.key, family.name
                                    ));
                                }
                                for name in std::iter::once(&member.name).chain(&member.aliases) {
                                    if !names.insert(normalize_quest_name(name)) {
                                        return Err(format!("quest name '{name}' is repeated"));
                                    }
                                }
                            }
                        }
                        CatalogueEntry::Quest(quest) => {
                            validate_reward(&quest.key, &quest.reward)?;
                            if quest.cooldown_hours <= 0.0 {
                                return Err(format!("quest '{}' needs a cooldown", quest.key));
                            }
                            let signal = quest
                                .signal_item
                                .as_deref()
                                .is_some_and(|item| !item.trim().is_empty());
                            if signal != (quest.completion == Completion::SignalItem) {
                                return Err(format!(
                                    "quest '{}' must name a signal item exactly when it completes on one",
                                    quest.key
                                ));
                            }
                            for name in std::iter::once(&quest.name).chain(&quest.aliases) {
                                if !names.insert(normalize_quest_name(name)) {
                                    return Err(format!("quest name '{name}' is repeated"));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn validate_reward(key: &str, reward: &CatalogueReward) -> Result<(), String> {
    let well_formed = match reward.policy {
        RewardPolicy::NamedItems => !reward.items.is_empty() && reward.amount.is_none(),
        RewardPolicy::FixedPes => {
            reward.items.is_empty() && reward.amount.is_some_and(|amount| amount > 0.0)
        }
        RewardPolicy::None | RewardPolicy::CompletionClump => {
            reward.items.is_empty() && reward.amount.is_none()
        }
    };
    if !well_formed || reward.label.trim().is_empty() {
        return Err(format!("'{key}' has a malformed reward"));
    }
    Ok(())
}

/// What one sync changed, for the startup log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub inserted: usize,
    pub adopted: usize,
    pub updated: usize,
    pub retired: usize,
}

/// The fields the catalogue owns on a quest row.
struct QuestFields<'a> {
    key: &'a str,
    name: &'a str,
    aliases: &'a [String],
    planet: &'a str,
    category: &'a str,
    waypoint: Option<&'a str>,
    cooldown_hours: Option<f64>,
    cooldown_anchor: Anchor,
    completion: Completion,
    signal_item: Option<&'a str>,
    reward: &'a CatalogueReward,
    family_id: Option<i64>,
}

/// Materialise [`CATALOGUE`] into the database, in one transaction.
pub async fn sync_catalogue(db: &Db, clock: &dyn Clock) -> Result<SyncReport, DbError> {
    let now = crate::time::instant_to_epoch(crate::time::resolve_local(clock.now()));
    db.with_writer(move |conn| {
        let tx = conn.transaction()?;
        let report = apply_catalogue(&tx, &CATALOGUE, now)?;
        tx.commit()?;
        Ok(report)
    })
    .await
}

/// The sync against an explicit catalogue and instant (the tests drive it
/// with their own fixtures).
pub fn apply_catalogue(
    tx: &rusqlite::Transaction<'_>,
    catalogue: &Catalogue,
    now: f64,
) -> rusqlite::Result<SyncReport> {
    let mut report = SyncReport::default();
    let mut quest_keys = Vec::new();
    let mut family_keys = Vec::new();
    for planet in &catalogue.planets {
        for category in &planet.categories {
            for entry in &category.entries {
                match entry {
                    CatalogueEntry::Family(family) => {
                        family_keys.push(family.key.clone());
                        let family_id = sync_family(tx, family, &planet.name, &mut report)?;
                        for member in &family.members {
                            quest_keys.push(member.key.clone());
                            sync_quest(
                                tx,
                                &QuestFields {
                                    key: &member.key,
                                    name: &member.name,
                                    aliases: &member.aliases,
                                    planet: &planet.name,
                                    category: &category.name,
                                    waypoint: member.waypoint.as_deref(),
                                    // The family is the slot, so it alone
                                    // carries the cooldown.
                                    cooldown_hours: None,
                                    cooldown_anchor: family.cooldown_anchor,
                                    completion: Completion::MissionLog,
                                    signal_item: None,
                                    reward: &family.reward,
                                    family_id: Some(family_id),
                                },
                                now,
                                &mut report,
                            )?;
                        }
                    }
                    CatalogueEntry::Quest(quest) => {
                        quest_keys.push(quest.key.clone());
                        sync_quest(
                            tx,
                            &QuestFields {
                                key: &quest.key,
                                name: &quest.name,
                                aliases: &quest.aliases,
                                planet: &planet.name,
                                category: &category.name,
                                waypoint: quest.waypoint.as_deref(),
                                cooldown_hours: Some(quest.cooldown_hours),
                                cooldown_anchor: quest.cooldown_anchor,
                                completion: quest.completion,
                                signal_item: quest.signal_item.as_deref(),
                                reward: &quest.reward,
                                family_id: None,
                            },
                            now,
                            &mut report,
                        )?;
                    }
                }
            }
        }
    }
    report.retired += retire_missing(tx, "quests", &quest_keys)?;
    report.retired += retire_missing(tx, "quest_families", &family_keys)?;
    Ok(report)
}

/// Find a row by key, or the best unkeyed row carrying one of `names`:
/// an active row first, then the one with the most recorded completions,
/// then the oldest. `None` when nothing matches.
fn find_row(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    key: &str,
    names: &[&str],
) -> rusqlite::Result<Option<(i64, bool)>> {
    use rusqlite::OptionalExtension as _;
    let keyed: Option<i64> = tx
        .query_row(
            &format!("SELECT id FROM {table} WHERE catalogue_key = ?"),
            rusqlite::params![key],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = keyed {
        return Ok(Some((id, false)));
    }
    let wanted: Vec<String> = names.iter().map(|name| normalize_quest_name(name)).collect();
    let completions = if table == "quests" {
        "(SELECT COUNT(*) FROM session_quest_completions c WHERE c.quest_id = t.id)"
    } else {
        "0"
    };
    let mut stmt = tx.prepare(&format!(
        "SELECT t.id, t.name, t.is_active, {completions} FROM {table} t \
         WHERE t.catalogue_key IS NULL"
    ))?;
    let mut rows = stmt.query([])?;
    let mut best: Option<(i64, i64, i64)> = None;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if !wanted.contains(&normalize_quest_name(&name)) {
            continue;
        }
        let candidate = (row.get::<_, i64>(0)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?);
        let better = match best {
            None => true,
            Some((id, active, count)) => {
                (candidate.1, candidate.2, -candidate.0) > (active, count, -id)
            }
        };
        if better {
            best = Some(candidate);
        }
    }
    Ok(best.map(|(id, _, _)| (id, true)))
}

fn sync_family(
    tx: &rusqlite::Transaction<'_>,
    family: &CatalogueFamily,
    planet: &str,
    report: &mut SyncReport,
) -> rusqlite::Result<i64> {
    match find_row(tx, "quest_families", &family.key, &[family.name.as_str()])? {
        Some((id, adopted)) => {
            let changed = tx.execute(
                "UPDATE quest_families \
                 SET catalogue_key = ?1, name = ?2, planet = ?3, cooldown_hours = ?4, \
                     cooldown_anchor = ?5, is_active = 1 \
                 WHERE id = ?6 AND (catalogue_key IS NOT ?1 OR name IS NOT ?2 \
                     OR planet IS NOT ?3 OR cooldown_hours IS NOT ?4 \
                     OR cooldown_anchor IS NOT ?5 OR is_active IS NOT 1)",
                rusqlite::params![
                    family.key,
                    family.name,
                    planet,
                    family.cooldown_hours,
                    family.cooldown_anchor.as_str(),
                    id
                ],
            )?;
            if adopted {
                report.adopted += 1;
            } else if changed > 0 {
                report.updated += 1;
            }
            Ok(id)
        }
        None => {
            tx.execute(
                "INSERT INTO quest_families \
                 (name, planet, cooldown_hours, cooldown_anchor, catalogue_key) \
                 VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![
                    family.name,
                    planet,
                    family.cooldown_hours,
                    family.cooldown_anchor.as_str(),
                    family.key
                ],
            )?;
            report.inserted += 1;
            Ok(tx.last_insert_rowid())
        }
    }
}

fn sync_quest(
    tx: &rusqlite::Transaction<'_>,
    fields: &QuestFields<'_>,
    now: f64,
    report: &mut SyncReport,
) -> rusqlite::Result<()> {
    let names: Vec<&str> = std::iter::once(fields.name)
        .chain(fields.aliases.iter().map(String::as_str))
        .collect();
    let reward_ped = match fields.reward.policy {
        RewardPolicy::FixedPes => fields.reward.amount,
        _ => None,
    };
    let reward_is_skill = i64::from(fields.reward.policy == RewardPolicy::FixedPes);
    let params = rusqlite::params![
        fields.key,
        fields.name,
        fields.planet,
        fields.category,
        fields.waypoint,
        fields.cooldown_hours,
        fields.cooldown_anchor.as_str(),
        fields.completion.as_str(),
        fields.signal_item,
        fields.reward.policy.as_str(),
        reward_ped,
        reward_is_skill,
        fields.reward.label,
        fields.family_id,
    ];
    let quest_id = match find_row(tx, "quests", fields.key, &names)? {
        Some((id, adopted)) => {
            let mut update_params: Vec<&dyn rusqlite::ToSql> = params.to_vec();
            update_params.push(&now);
            update_params.push(&id);
            let changed = tx.execute(
                "UPDATE quests \
                 SET catalogue_key = ?1, name = ?2, planet = ?3, category = ?4, waypoint = ?5, \
                     cooldown_hours = ?6, cooldown_anchor = ?7, completion_mode = ?8, \
                     completion_trigger = ?8, signal_loot_item = ?9, reward_policy = ?10, \
                     reward_ped = ?11, reward_is_skill = ?12, reward_description = ?13, \
                     family_id = ?14, is_active = 1, updated_at = ?15 \
                 WHERE id = ?16 AND (catalogue_key IS NOT ?1 OR name IS NOT ?2 \
                     OR planet IS NOT ?3 OR category IS NOT ?4 OR waypoint IS NOT ?5 \
                     OR cooldown_hours IS NOT ?6 OR cooldown_anchor IS NOT ?7 \
                     OR completion_mode IS NOT ?8 OR completion_trigger IS NOT ?8 \
                     OR signal_loot_item IS NOT ?9 OR reward_policy IS NOT ?10 \
                     OR reward_ped IS NOT ?11 OR reward_is_skill IS NOT ?12 \
                     OR reward_description IS NOT ?13 OR family_id IS NOT ?14 \
                     OR is_active IS NOT 1)",
                update_params.as_slice(),
            )?;
            if adopted {
                report.adopted += 1;
            } else if changed > 0 {
                report.updated += 1;
            }
            id
        }
        None => {
            tx.execute(
                "INSERT INTO quests \
                 (catalogue_key, name, planet, category, waypoint, cooldown_hours, \
                  cooldown_anchor, completion_mode, completion_trigger, signal_loot_item, \
                  reward_policy, reward_ped, reward_is_skill, reward_description, family_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params,
            )?;
            report.inserted += 1;
            tx.last_insert_rowid()
        }
    };
    sync_reward_items(tx, quest_id, &fields.reward.items)
}

/// Replace a quest's named reward items only when they differ, so an
/// unchanged catalogue writes nothing.
fn sync_reward_items(
    tx: &rusqlite::Transaction<'_>,
    quest_id: i64,
    items: &[String],
) -> rusqlite::Result<()> {
    let current: Vec<String> = tx
        .prepare(
            "SELECT item_name FROM quest_reward_item_rules \
             WHERE quest_id = ? ORDER BY sort_order, item_name",
        )?
        .query_map(rusqlite::params![quest_id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if current == items {
        return Ok(());
    }
    tx.execute(
        "DELETE FROM quest_reward_item_rules WHERE quest_id = ?",
        rusqlite::params![quest_id],
    )?;
    for (sort_order, item) in items.iter().enumerate() {
        tx.execute(
            "INSERT INTO quest_reward_item_rules (quest_id, item_name, sort_order) \
             VALUES (?, ?, ?)",
            rusqlite::params![quest_id, item, sort_order as i64],
        )?;
    }
    Ok(())
}

/// Soft-delete keyed rows the catalogue no longer carries.
fn retire_missing(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    keys: &[String],
) -> rusqlite::Result<usize> {
    let keep: BTreeSet<&str> = keys.iter().map(String::as_str).collect();
    let keyed: Vec<(i64, String)> = tx
        .prepare(&format!(
            "SELECT id, catalogue_key FROM {table} \
             WHERE catalogue_key IS NOT NULL AND is_active = 1"
        ))?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut retired = 0;
    for (id, key) in keyed {
        if !keep.contains(key.as_str()) {
            retired += tx.execute(
                &format!("UPDATE {table} SET is_active = 0 WHERE id = ?"),
                rusqlite::params![id],
            )?;
        }
    }
    Ok(retired)
}
