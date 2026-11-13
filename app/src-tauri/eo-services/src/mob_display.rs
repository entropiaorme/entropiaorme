//! How a mob is shown: the one place a mob's displayed name is made.
//!
//! A mob is recorded by what it is (its name and maturity); what the
//! application shows for it is a label made here. By default that is the
//! name as the game prints it on the mob's plate: the name, then the
//! maturity (`Atrox Young`, `Survival Zombie Dog 04`). The player may give
//! a mob another label, where the bundled list's name is not quite the
//! game's; the label changes nothing about which mob it is, so kills
//! recorded under either label stay one mob's.
//!
//! The labels the player chose live in `mob_display_names` and are held
//! here in memory for the tracker, which names kills as they are recorded.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::db::DbError;

/// The longest label a mob may be given.
pub const MAX_LABEL_CHARS: usize = 64;

/// A mob's name as the game prints it: its name, then its maturity when
/// it has one.
pub fn printed(name: &str, maturity: &str) -> String {
    if maturity.is_empty() {
        name.to_string()
    } else {
        format!("{name} {maturity}")
    }
}

/// One label the player chose: the mob (name and maturity as recorded)
/// and what to show for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobLabel {
    pub species: String,
    pub maturity: String,
    pub label: String,
}

/// The labels the player chose, as the tracker reads them.
#[derive(Debug, Default)]
pub struct MobDisplayNames {
    labels: RwLock<HashMap<(String, String), String>>,
}

impl MobDisplayNames {
    /// The label the player chose for the mob, when they chose one.
    pub fn chosen(&self, species: &str, maturity: &str) -> Option<String> {
        self.labels
            .read()
            .ok()?
            .get(&(species.to_string(), maturity.to_string()))
            .cloned()
    }

    /// What to show for the mob: the player's label, else its name as the
    /// game prints it, with `printed_maturity` (the maturity as the plate
    /// prints it, `04` where it is recorded as `4`) when one is known.
    pub fn shown(&self, species: &str, maturity: &str, printed_maturity: Option<&str>) -> String {
        self.chosen(species, maturity)
            .unwrap_or_else(|| printed(species, printed_maturity.unwrap_or(maturity)))
    }

    /// Hold `labels` in place of those held.
    pub fn replace(&self, labels: Vec<MobLabel>) {
        if let Ok(mut held) = self.labels.write() {
            *held = labels
                .into_iter()
                .map(|l| ((l.species, l.maturity), l.label))
                .collect();
        }
    }
}

/// Every label the player chose, by mob.
pub fn labels(conn: &rusqlite::Connection) -> Result<Vec<MobLabel>, DbError> {
    let mut statement = conn.prepare(
        "SELECT species, maturity, label FROM mob_display_names ORDER BY species, maturity",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(MobLabel {
            species: row.get(0)?,
            maturity: row.get(1)?,
            label: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Who gave a mob its label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSource {
    /// How the mob's plate prints it, read off the screen.
    Plate,
    /// The player's own choice, which a reading never replaces.
    Player,
}

impl LabelSource {
    fn as_str(self) -> &'static str {
        match self {
            LabelSource::Plate => "plate",
            LabelSource::Player => "player",
        }
    }
}

/// Show the mob as `label` from now on, and everywhere it is already
/// recorded: the label is kept, and every stored name of the mob is rewritten
/// to it, with the summaries of the ended sessions it was killed in. A label
/// read off a plate gives way to one the player chose. Answers whether
/// anything changed.
pub fn set_label(
    tx: &rusqlite::Transaction<'_>,
    species: &str,
    maturity: &str,
    label: &str,
    source: LabelSource,
    now: f64,
) -> Result<bool, DbError> {
    use rusqlite::OptionalExtension as _;
    let held: Option<(String, String)> = tx
        .query_row(
            "SELECT label, source FROM mob_display_names WHERE species = ? AND maturity = ?",
            rusqlite::params![species, maturity],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match &held {
        Some((_, by)) if source == LabelSource::Plate && by == LabelSource::Player.as_str() => {
            return Ok(false)
        }
        Some((text, by)) if text == label && by == source.as_str() => return Ok(false),
        _ => {}
    }
    tx.execute(
        "INSERT INTO mob_display_names (species, maturity, label, source, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(species, maturity) DO UPDATE SET \
             label = excluded.label, source = excluded.source, updated_at = excluded.updated_at",
        rusqlite::params![species, maturity, label, source.as_str(), now],
    )?;
    rename(tx, species, maturity, label)?;
    Ok(true)
}

/// Forget the mob's label: it is shown as the game prints it again,
/// everywhere it is recorded. Answers whether it had one.
pub fn clear_label(
    tx: &rusqlite::Transaction<'_>,
    species: &str,
    maturity: &str,
) -> Result<bool, DbError> {
    let cleared = tx.execute(
        "DELETE FROM mob_display_names WHERE species = ? AND maturity = ?",
        rusqlite::params![species, maturity],
    )?;
    if cleared > 0 {
        rename(tx, species, maturity, &printed(species, maturity))?;
    }
    Ok(cleared > 0)
}

/// Rewrite every stored name of the mob to `label`, and the summaries of
/// the ended sessions whose kills were renamed.
fn rename(
    tx: &rusqlite::Transaction<'_>,
    species: &str,
    maturity: &str,
    label: &str,
) -> Result<(), DbError> {
    let sessions: Vec<String> = {
        let mut statement = tx.prepare(
            "SELECT DISTINCT k.session_id FROM kills k \
             JOIN session_summaries s ON s.session_id = k.session_id \
             WHERE k.mob_species = ?1 AND COALESCE(k.mob_maturity, '') = ?2 \
               AND k.mob_name IS NOT ?3",
        )?;
        let rows = statement.query_map(rusqlite::params![species, maturity, label], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect::<Result<_, _>>()?
    };
    tx.execute(
        "UPDATE kills SET mob_name = ?3 \
         WHERE mob_species = ?1 AND COALESCE(mob_maturity, '') = ?2 AND mob_name IS NOT ?3",
        rusqlite::params![species, maturity, label],
    )?;
    for table in ["kill_mob_detections", "kill_mob_carried"] {
        for (name, of_species, of_maturity) in [
            ("mob_name", "mob_species", "mob_maturity"),
            ("declared_name", "declared_species", "declared_maturity"),
            ("settled_name", "settled_species", "settled_maturity"),
        ] {
            tx.execute(
                &format!(
                    "UPDATE {table} SET {name} = ?3 \
                     WHERE {of_species} = ?1 AND {of_maturity} = ?2 AND {name} <> ?3"
                ),
                rusqlite::params![species, maturity, label],
            )?;
        }
    }
    tx.execute(
        "UPDATE kill_mob_assigned SET settled_name = ?3 \
         WHERE settled_species = ?1 AND settled_maturity = ?2 AND settled_name <> ?3",
        rusqlite::params![species, maturity, label],
    )?;
    tx.execute(
        "UPDATE kill_mob_assigned SET declared_name = ?3 \
         WHERE declared_species = ?1 AND declared_maturity = ?2 AND declared_name <> ?3",
        rusqlite::params![species, maturity, label],
    )?;
    for session_id in sessions {
        crate::session_summary::write_session_summary(tx, &session_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mob_is_shown_as_the_game_prints_it_unless_the_player_chose_a_label() {
        let names = MobDisplayNames::default();
        assert_eq!(names.shown("Atrox", "Young", None), "Atrox Young");
        assert_eq!(names.shown("Feffoid", "", None), "Feffoid");
        assert_eq!(
            names.shown("Survival Zombie Dog", "4", Some("04")),
            "Survival Zombie Dog 04"
        );
        names.replace(vec![MobLabel {
            species: "Survival Zombie Dog".into(),
            maturity: "4".into(),
            label: "Zombie Dog IV".into(),
        }]);
        assert_eq!(
            names.shown("Survival Zombie Dog", "4", Some("04")),
            "Zombie Dog IV"
        );
        assert_eq!(names.chosen("Survival Zombie Dog", "5"), None);
        names.replace(Vec::new());
        assert_eq!(
            names.shown("Survival Zombie Dog", "4", None),
            "Survival Zombie Dog 4"
        );
    }
}
