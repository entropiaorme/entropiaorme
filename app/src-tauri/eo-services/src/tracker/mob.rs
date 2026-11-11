//! The declared-mob layer: the per-kill mob stamp's vocabulary and the
//! commands that drive the declaration. The declared mob is one of the
//! session's independent attributions (beside the session-name and
//! skill-boost facets, `session::SessionFacets`), changeable
//! mid-session. It feeds the per-kill stamp whenever detection (see
//! `detection`) does not name the creature a kill was.

use super::actor::TrackerActor;
use super::TrackerCommandError;

/// Where a kill's mob stamp came from. `Declared` is the player's word:
/// the declaration in force when the kill settled, or a choice made when
/// reviewing the session; `Detected` is the creature read off the game
/// screen. A kill recorded with no declaration in force and nothing
/// detected carries no source (its stamp is "Unknown").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobStampSource {
    Declared,
    Detected,
}

impl MobStampSource {
    /// The wire/database string.
    pub fn as_str(self) -> &'static str {
        match self {
            MobStampSource::Declared => "declared",
            MobStampSource::Detected => "detected",
        }
    }
}

/// How sure detection was of the creature it gave a kill, from the name
/// over the creature the player was locked on to. The upper four grades
/// stamp the kill; the two lowest are kept only as suggestions, since the
/// creature read last before a kill is wrong too often to record as the
/// kill's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MobDetectionGrade {
    /// A listed creature, read whole and repeatedly in its own fight.
    Named,
    /// A listed creature the screen leans to without proving it.
    Guess,
    /// A creature on no list, by the spelling read most often.
    Provisional,
    /// A creature on no list, leaned to but not proved.
    ProvisionalGuess,
    /// The listed creature read last before the kill, where the screen
    /// showed no lean.
    LatestRead,
    /// The creature on no list read last before the kill.
    ProvisionalLatestRead,
}

impl MobDetectionGrade {
    pub const ALL: [MobDetectionGrade; 6] = [
        MobDetectionGrade::Named,
        MobDetectionGrade::Guess,
        MobDetectionGrade::Provisional,
        MobDetectionGrade::ProvisionalGuess,
        MobDetectionGrade::LatestRead,
        MobDetectionGrade::ProvisionalLatestRead,
    ];

    /// The database string.
    pub fn as_str(self) -> &'static str {
        match self {
            MobDetectionGrade::Named => "named",
            MobDetectionGrade::Guess => "guess",
            MobDetectionGrade::Provisional => "provisional",
            MobDetectionGrade::ProvisionalGuess => "provisional_guess",
            MobDetectionGrade::LatestRead => "latest_read",
            MobDetectionGrade::ProvisionalLatestRead => "provisional_latest_read",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|grade| grade.as_str() == value)
    }

    /// Whether a kill takes the creature at this grade as its stamp;
    /// otherwise it is a suggestion only.
    pub fn stamps(self) -> bool {
        !matches!(
            self,
            MobDetectionGrade::LatestRead | MobDetectionGrade::ProvisionalLatestRead
        )
    }

    /// Whether the creature is one on no list, by the session's own name.
    pub fn provisional(self) -> bool {
        matches!(
            self,
            MobDetectionGrade::Provisional
                | MobDetectionGrade::ProvisionalGuess
                | MobDetectionGrade::ProvisionalLatestRead
        )
    }
}

/// The mob the session currently declares it is hunting: the stamp
/// source for kills recorded while it is in force. Display name and
/// catalogue identity always travel together, so a stamped name
/// without its species/maturity pair is unrepresentable.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredMob {
    /// The display name ("<maturity> <species>", or the bare species
    /// when no maturity is set).
    pub name: String,
    pub species: String,
    pub maturity: String,
}

impl DeclaredMob {
    /// Build the declaration from a species/maturity pair, deriving
    /// the display name the way the session-start and reload paths
    /// always have.
    pub(crate) fn from_parts(species: String, maturity: String) -> Self {
        let name = if maturity.is_empty() {
            species.clone()
        } else {
            format!("{maturity} {species}")
        };
        DeclaredMob {
            name,
            species,
            maturity,
        }
    }
}

impl TrackerActor {
    /// Immediately set the declared mob for kill stamping. Mid-session
    /// changes are allowed by design: the declaration is the current
    /// stamp source, not a session-frozen mode.
    pub(super) fn set_declared_mob(
        &mut self,
        mob_name: &str,
        species: &str,
        maturity: &str,
    ) -> Result<(), TrackerCommandError> {
        let Some(active) = self.session.active_mut() else {
            return Err(TrackerCommandError::NoActiveSession);
        };
        active.declared_mob = Some(DeclaredMob {
            name: mob_name.to_string(),
            species: species.to_string(),
            maturity: maturity.to_string(),
        });
        Ok(())
    }

    /// Clear the declared mob, returning the released name. Idle is a
    /// no-op (idle carries no declaration to release).
    pub(super) fn release_declared_mob(&mut self) -> Option<String> {
        let active = self.session.active_mut()?;
        active
            .declared_mob
            .take()
            .map(|declared| declared.name)
            .filter(|name| !name.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mob_stamp_source_wire_strings() {
        assert_eq!(MobStampSource::Declared.as_str(), "declared");
        assert_eq!(MobStampSource::Detected.as_str(), "detected");
    }

    #[test]
    fn detection_grades_round_trip_and_say_which_stamp() {
        for grade in MobDetectionGrade::ALL {
            assert_eq!(MobDetectionGrade::parse(grade.as_str()), Some(grade));
        }
        assert_eq!(MobDetectionGrade::parse("Named"), None);
        let stamping: Vec<_> = MobDetectionGrade::ALL
            .into_iter()
            .filter(|grade| grade.stamps())
            .collect();
        assert_eq!(
            stamping,
            [
                MobDetectionGrade::Named,
                MobDetectionGrade::Guess,
                MobDetectionGrade::Provisional,
                MobDetectionGrade::ProvisionalGuess,
            ]
        );
        assert!(MobDetectionGrade::ProvisionalLatestRead.provisional());
        assert!(!MobDetectionGrade::LatestRead.provisional());
    }

    #[test]
    fn declared_mob_display_name_derivation() {
        let bare = DeclaredMob::from_parts("Atrox".into(), String::new());
        assert_eq!(bare.name, "Atrox");
        let mature = DeclaredMob::from_parts("Atrox".into(), "Old".into());
        assert_eq!(mature.name, "Old Atrox");
    }
}
