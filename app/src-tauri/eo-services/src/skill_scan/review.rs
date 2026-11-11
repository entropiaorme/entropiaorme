//! What a finished sweep of the skills window comes to for the player:
//! what accepting it saves, and what they should know before they do.
//!
//! Only a skill read on its page under a name the bundled list knows,
//! and read once, is ever saved: spelled whole, or a near miss of
//! exactly one skill. Everything else is shown and never saved: a skill
//! the list lacks has nowhere to go, a name the sweep could only guess at
//! might be another skill, and a name that could be either of two skills,
//! or a skill read twice, cannot say which skill its points belong to.
//! Each saved level carries the moment its page was read, so a level the
//! chat log records later is never overwritten by an older reading.

use std::collections::BTreeMap;

use crate::game_ui::skills::{Checksum, NameReading, SweepResult, UnknownSkill};
use crate::scan_completion::ScannedLevel;

/// A skill the game shows whose points are not saved.
#[derive(Debug, Clone, PartialEq)]
pub struct UnsavedSkill {
    /// The name as it was read.
    pub name: String,
    /// The points exactly as the game prints them.
    pub points_text: String,
}

impl From<&UnknownSkill> for UnsavedSkill {
    fn from(skill: &UnknownSkill) -> Self {
        Self {
            name: skill.raw_name.trim().to_string(),
            points_text: skill.points_text.clone(),
        }
    }
}

/// A saved skill whose name was not spelled whole on its page: a slip of
/// a character or two from it and from no other skill, or a name the
/// game cut short that only it begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearMissSkill {
    /// The skill it was saved as, as the bundled list spells it.
    pub skill: String,
    /// What the row was read as.
    pub read_as: String,
}

/// A name the bundled list does not know, which is probably one of its
/// skills: the nearest of those the scan did not otherwise read. Never
/// saved, since a skill the list lacks can look just as near.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuessedSkill {
    /// What the row was read as.
    pub read_as: String,
    /// The skill it probably is.
    pub probably: String,
    /// The points exactly as the game prints them.
    pub points_text: String,
}

/// Whether the figures read add up to the total the game prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotalCheck {
    /// The whole points of every skill read add up to the game's total.
    Matches,
    /// They do not: some figure was misread, a row was missed, or a skill
    /// gained a whole point between pages.
    Mismatch { sum: u64, total: u64 },
    /// There was nothing to check against: no total read, the total
    /// changed while the pages were read, or the rows are not one list
    /// (a skill read twice, or a name that could be either of two).
    Unchecked,
}

impl From<Checksum> for TotalCheck {
    fn from(checksum: Checksum) -> Self {
        match checksum {
            Checksum::Matches => TotalCheck::Matches,
            Checksum::Mismatch { sum, total } => TotalCheck::Mismatch { sum, total },
            Checksum::Unverified | Checksum::Inconsistent => TotalCheck::Unchecked,
        }
    }
}

/// A finished scan, held until the player accepts or rejects it.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillScanReview {
    /// The scan it came from: accepting or rejecting names it, so a
    /// review on screen can only ever settle the scan it shows.
    pub scan: u64,
    /// What accepting saves, in the game's order, each level stamped with
    /// when its page was read.
    pub skills: Vec<ScannedLevel>,
    pub total: TotalCheck,
    /// Saved skills whose names were not read exactly.
    pub near_misses: Vec<NearMissSkill>,
    /// Names the list does not know that are probably one of its skills.
    pub guesses: Vec<GuessedSkill>,
    /// Skills the game shows that this version does not know.
    pub unknown: Vec<UnsavedSkill>,
    /// Rows whose name comes equally near two skills.
    pub unplaced: Vec<UnsavedSkill>,
    /// Skills read more than once.
    pub repeated: Vec<String>,
}

impl SkillScanReview {
    /// The review of `result`, whose page `n` was read at `read_at[n]`
    /// (epoch seconds).
    pub fn from_sweep(scan: u64, result: &SweepResult, read_at: &BTreeMap<u32, f64>) -> Self {
        let repeated: Vec<String> = result.duplicates.iter().flatten().cloned().collect();
        let mut skills = Vec::new();
        let mut near_misses = Vec::new();
        let mut guesses = Vec::new();
        for entry in &result.skills {
            // The client's blank row has no skill to save.
            let Some(name) = &entry.name else {
                continue;
            };
            if repeated.contains(name) {
                continue;
            }
            let read_as = entry.raw_name.trim().to_string();
            match entry.reading {
                NameReading::Spelled => {}
                NameReading::NearMiss => near_misses.push(NearMissSkill {
                    skill: name.clone(),
                    read_as,
                }),
                NameReading::Likely => {
                    guesses.push(GuessedSkill {
                        read_as,
                        probably: name.clone(),
                        points_text: entry.points_text.clone(),
                    });
                    continue;
                }
            }
            // Every kept page was stamped when it was read, so a page with
            // no stamp cannot occur; were one to, its skills are not saved
            // rather than saved with a made-up time.
            let Some(&scanned_at) = read_at.get(&entry.page) else {
                continue;
            };
            skills.push(ScannedLevel {
                skill: name.clone(),
                level: entry.points,
                scanned_at,
            });
        }
        Self {
            scan,
            skills,
            total: result.checksum.into(),
            near_misses,
            guesses,
            unknown: result.unknown.iter().map(UnsavedSkill::from).collect(),
            unplaced: result.unresolved.iter().map(UnsavedSkill::from).collect(),
            repeated,
        }
    }

    /// Whether the figures fall short of a faithful picture of the game's
    /// list: they do not add up to its total, or could not be checked
    /// against it. Such a review is saved only when the player says so.
    pub fn needs_confirmation(&self) -> bool {
        self.total != TotalCheck::Matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::skills::SweepEntry;

    fn entry(
        name: Option<&str>,
        raw: &str,
        points: f64,
        reading: NameReading,
        page: u32,
    ) -> SweepEntry {
        SweepEntry {
            name: name.map(String::from),
            reading,
            raw_name: raw.to_string(),
            points_text: points.to_string(),
            points,
            page,
        }
    }

    fn unknown(raw: &str, points: f64) -> UnknownSkill {
        UnknownSkill {
            raw_name: raw.to_string(),
            points_text: points.to_string(),
            points,
            page: 2,
        }
    }

    fn result() -> SweepResult {
        SweepResult {
            category: Some("ALL CATEGORIES".into()),
            skills: vec![
                entry(None, "", 1.0, NameReading::Spelled, 1),
                entry(Some("Agility"), "Agility", 73.71, NameReading::Spelled, 1),
                entry(Some("Aim"), " Aim. ", 1268.53, NameReading::NearMiss, 2),
                entry(Some("Anatomy"), "Anatomv", 40.0, NameReading::Likely, 2),
            ],
            unknown: vec![unknown("Food Technology ", 5.25)],
            unresolved: Vec::new(),
            duplicates: Vec::new(),
            total: Some(1387),
            checksum: Checksum::Matches,
        }
    }

    fn read_at() -> BTreeMap<u32, f64> {
        BTreeMap::from([(1, 100.0), (2, 160.0)])
    }

    fn level(skill: &str, level: f64, scanned_at: f64) -> ScannedLevel {
        ScannedLevel {
            skill: skill.into(),
            level,
            scanned_at,
        }
    }

    #[test]
    fn only_skills_read_once_by_a_known_name_are_saved_each_with_its_page_time() {
        let review = SkillScanReview::from_sweep(7, &result(), &read_at());
        assert_eq!(review.scan, 7);
        assert_eq!(
            review.skills,
            vec![level("Agility", 73.71, 100.0), level("Aim", 1268.53, 160.0)],
            "the blank row, the guess and the unknown skill are not saved"
        );
        assert_eq!(review.total, TotalCheck::Matches);
        assert!(!review.needs_confirmation());
        assert_eq!(
            review.near_misses,
            vec![NearMissSkill {
                skill: "Aim".into(),
                read_as: "Aim.".into()
            }]
        );
        assert_eq!(
            review.guesses,
            vec![GuessedSkill {
                read_as: "Anatomv".into(),
                probably: "Anatomy".into(),
                points_text: "40".into(),
            }]
        );
        assert_eq!(review.unknown.len(), 1);
        assert_eq!(review.unknown[0].name, "Food Technology");
        assert!(review.unplaced.is_empty() && review.repeated.is_empty());
    }

    #[test]
    fn a_skill_read_twice_is_not_saved_at_all() {
        let mut result = result();
        result.skills.push(entry(
            Some("Agility"),
            "Agility",
            80.0,
            NameReading::Spelled,
            2,
        ));
        result.duplicates = vec![Some("Agility".into()), None];
        result.unresolved = vec![unknown("Arn", 9.0)];
        result.checksum = Checksum::Inconsistent;
        let review = SkillScanReview::from_sweep(1, &result, &read_at());
        assert_eq!(review.skills, vec![level("Aim", 1268.53, 160.0)]);
        assert_eq!(review.repeated, vec!["Agility".to_string()]);
        assert_eq!(review.unplaced[0].name, "Arn");
        assert_eq!(review.total, TotalCheck::Unchecked);
        assert!(review.needs_confirmation());
    }

    #[test]
    fn each_checksum_comes_to_what_the_player_is_told() {
        assert_eq!(TotalCheck::from(Checksum::Matches), TotalCheck::Matches);
        assert_eq!(
            TotalCheck::from(Checksum::Mismatch { sum: 4, total: 5 }),
            TotalCheck::Mismatch { sum: 4, total: 5 }
        );
        assert_eq!(
            TotalCheck::from(Checksum::Unverified),
            TotalCheck::Unchecked
        );
        assert_eq!(
            TotalCheck::from(Checksum::Inconsistent),
            TotalCheck::Unchecked
        );
        let mut mismatched = result();
        mismatched.checksum = Checksum::Mismatch { sum: 1, total: 2 };
        assert!(SkillScanReview::from_sweep(1, &mismatched, &read_at()).needs_confirmation());
    }
}
