//! The session's mob source: the per-kill mob stamp's vocabulary, the
//! one source a session names its kills' creature by, and the commands
//! that steer it. The source is one of the session's independent
//! attributions (beside the session-name and skill-boost facets,
//! `session::SessionFacets`), changeable mid-session.
//!
//! The source holds the creature the player typed (the declared mob of
//! old, kept in the settings so it carries into the next session) and,
//! when the player pinned one, the pin. How a kill is named follows from
//! them and from whether the screen is being read (see `detection`):
//!
//! - pinned: every kill takes the pinned creature, and nothing is read
//!   off the screen while the pin stands. A pin is the player's word: it
//!   stands, whether detection is on or off, until the player releases it
//!   or types another creature (which replaces it, pinned by typing);
//! - reading the screen, unpinned: the screen's evidence for the kill,
//!   then the creature being hunted (continuity), then the typed creature
//!   as a backstop, else no creature;
//! - not reading the screen, unpinned: the typed creature is pinned (by
//!   typing), else no creature. This is exactly the declared mob as it
//!   always was, so typing a creature while detection is off pins it, and
//!   typing one while the screen is read sets the backstop without
//!   pinning.

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

/// How detection named the creature it gave a kill. The upper four are
/// the screen's own evidence for the kill and stamp it; the two lowest are
/// kept only as suggestions, since the creature read last before a kill is
/// wrong too often to record as the kill's own; the two continuity grades
/// are the creature being hunted, given where the screen could not settle
/// the kill, and stamp it too. The pinned grade is a creature on no list the
/// player pinned from what the screen read: kept so its spelling can be
/// confirmed after the session.
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
    /// The listed creature the latest kills were, where the screen could
    /// not settle this one.
    Continuity,
    /// The creature on no list the latest kills were.
    ProvisionalContinuity,
    /// A creature on no list, pinned from what the screen read.
    ProvisionalPinned,
}

impl MobDetectionGrade {
    pub const ALL: [MobDetectionGrade; 9] = [
        MobDetectionGrade::Named,
        MobDetectionGrade::Guess,
        MobDetectionGrade::Provisional,
        MobDetectionGrade::ProvisionalGuess,
        MobDetectionGrade::LatestRead,
        MobDetectionGrade::ProvisionalLatestRead,
        MobDetectionGrade::Continuity,
        MobDetectionGrade::ProvisionalContinuity,
        MobDetectionGrade::ProvisionalPinned,
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
            MobDetectionGrade::Continuity => "continuity",
            MobDetectionGrade::ProvisionalContinuity => "provisional_continuity",
            MobDetectionGrade::ProvisionalPinned => "provisional_pinned",
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
                | MobDetectionGrade::ProvisionalContinuity
                | MobDetectionGrade::ProvisionalPinned
        )
    }

    /// Whether the grade is the creature being hunted: it rests on earlier
    /// kills, not on anything read for this one.
    pub fn continuity(self) -> bool {
        matches!(
            self,
            MobDetectionGrade::Continuity | MobDetectionGrade::ProvisionalContinuity
        )
    }

    /// Whether the answer rests on nothing read for the kill itself (the
    /// creature being hunted, or a pin), so it is kept without a reading,
    /// apart from the screen's answers.
    pub fn carried(self) -> bool {
        self.continuity() || self == MobDetectionGrade::ProvisionalPinned
    }
}

/// How a kill's creature was named while its session ran: a closed
/// vocabulary, stored on the kill (`kills.mob_route`). A kill named by
/// nothing has no route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MobRoute {
    /// The screen's own evidence for the kill, at one of the four grades
    /// that stamp.
    Named,
    Guess,
    Provisional,
    ProvisionalGuess,
    /// The creature being hunted, where the screen could not settle the
    /// kill.
    Continuity,
    ProvisionalContinuity,
    /// The creature pinned from what the screen read.
    PinnedRead,
    /// The creature pinned by typing it (and, without screen reading, the
    /// typed creature).
    PinnedTyped,
    /// The typed backstop, where nothing else named the kill.
    Backstop,
}

impl MobRoute {
    pub const ALL: [MobRoute; 9] = [
        MobRoute::Named,
        MobRoute::Guess,
        MobRoute::Provisional,
        MobRoute::ProvisionalGuess,
        MobRoute::Continuity,
        MobRoute::ProvisionalContinuity,
        MobRoute::PinnedRead,
        MobRoute::PinnedTyped,
        MobRoute::Backstop,
    ];

    /// The database string.
    pub fn as_str(self) -> &'static str {
        match self {
            MobRoute::Named => "named",
            MobRoute::Guess => "guess",
            MobRoute::Provisional => "provisional",
            MobRoute::ProvisionalGuess => "provisional_guess",
            MobRoute::Continuity => "continuity",
            MobRoute::ProvisionalContinuity => "provisional_continuity",
            MobRoute::PinnedRead => "pinned_read",
            MobRoute::PinnedTyped => "pinned_typed",
            MobRoute::Backstop => "backstop",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|route| route.as_str() == value)
    }

    /// The route a detection answer at `grade` names a kill by; `None` for
    /// a suggestion, which names no kill.
    pub fn of_grade(grade: MobDetectionGrade) -> Option<Self> {
        Some(match grade {
            MobDetectionGrade::Named => MobRoute::Named,
            MobDetectionGrade::Guess => MobRoute::Guess,
            MobDetectionGrade::Provisional => MobRoute::Provisional,
            MobDetectionGrade::ProvisionalGuess => MobRoute::ProvisionalGuess,
            MobDetectionGrade::Continuity => MobRoute::Continuity,
            MobDetectionGrade::ProvisionalContinuity => MobRoute::ProvisionalContinuity,
            MobDetectionGrade::ProvisionalPinned => MobRoute::PinnedRead,
            MobDetectionGrade::LatestRead | MobDetectionGrade::ProvisionalLatestRead => {
                return None
            }
        })
    }

    /// The route of a kill taking the pin `by` made.
    pub fn of_pin(by: PinnedBy) -> Self {
        match by {
            PinnedBy::Read => MobRoute::PinnedRead,
            PinnedBy::Typed => MobRoute::PinnedTyped,
        }
    }
}

/// How the player pinned a creature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedBy {
    /// The creature the screen was reading when the player pinned it.
    Read,
    /// A creature the player typed.
    Typed,
}

/// A pinned creature: every kill takes it, and the screen is not read,
/// until the player releases it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub mob: DeclaredMob,
    pub by: PinnedBy,
    /// The session's key for a creature on no list pinned from what the
    /// screen read: its spelling is the screen's, so each kill pinned to it
    /// is kept for the review to confirm. `None` for a listed or confirmed
    /// creature, and for every creature pinned by typing (only a listed one
    /// can be typed).
    pub provisional_key: Option<String>,
}

/// The session's one source for its kills' creature (see the module doc).
///
/// A pin, when one stands, names every kill, whether the screen is read or
/// not: it is the player's word and stands until they release it. Releasing
/// it clears the pin alone; the typed creature then names kills again (the
/// backstop while the screen is read, otherwise pinned by typing, exactly as
/// the declared mob did). Typing a creature while a pin stands replaces the
/// pin with it (pinned by typing): it never sets a backstop silently behind
/// a pin.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MobSource {
    /// The creature the player typed, as the settings keep it: the
    /// backstop while the screen is read, and otherwise every kill's
    /// creature (pinned by typing).
    pub typed: Option<DeclaredMob>,
    /// The creature the player pinned, if any.
    pub pin: Option<Pin>,
}

/// The session's mob source as the readouts show it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MobSourceView {
    /// Whether the screen is being read for the session now: without it
    /// there is no creature read, nor one the latest kills were, and the
    /// typed creature is pinned.
    pub reading_screen: bool,
    pub pin: Option<Pin>,
    /// The typed creature: the backstop while the screen is read.
    pub typed: Option<DeclaredMob>,
    /// The creature the screen reads now, held through a moment's
    /// unclear reading.
    pub reading: Option<DeclaredMob>,
    /// The creature the latest kills were, when continuity would give it
    /// and what is read now does not contradict it.
    pub continuity: Option<DeclaredMob>,
}

/// The creature a kill is recorded with when it settles, before the screen
/// has answered for it: its stamp's three mob columns, where the stamp came
/// from and the route. `None` route: no creature ("Unknown").
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecordedMob {
    pub(crate) mob: Option<DeclaredMob>,
    pub(crate) route: Option<MobRoute>,
    /// For a creature on no list pinned from a reading: its key, and the
    /// stamp the kill would have had without the pin (for the review to put
    /// back).
    pub(crate) unlisted_pin: Option<(String, Option<DeclaredMob>)>,
}

impl MobSource {
    /// The typed creature, when it names one (an empty name behaves as
    /// unset, as the declared mob always has).
    fn typed_mob(&self) -> Option<&DeclaredMob> {
        self.typed.as_ref().filter(|typed| !typed.name.is_empty())
    }

    /// What a kill settling now is recorded with: the pin; else, while the
    /// screen is read (`reading_screen`), the typed backstop, which the
    /// screen's answer may later replace; else the typed creature, pinned
    /// by typing.
    pub(crate) fn record(&self, reading_screen: bool) -> RecordedMob {
        if let Some(pin) = &self.pin {
            return RecordedMob {
                mob: Some(pin.mob.clone()),
                route: Some(MobRoute::of_pin(pin.by)),
                unlisted_pin: pin
                    .provisional_key
                    .clone()
                    .map(|key| (key, self.typed_mob().cloned())),
            };
        }
        match self.typed_mob() {
            Some(typed) => RecordedMob {
                mob: Some(typed.clone()),
                route: Some(if reading_screen {
                    MobRoute::Backstop
                } else {
                    MobRoute::PinnedTyped
                }),
                unlisted_pin: None,
            },
            None => RecordedMob {
                mob: None,
                route: None,
                unlisted_pin: None,
            },
        }
    }
}

/// The mob the session currently declares it is hunting: the stamp
/// source for kills recorded while it is in force. Display name and
/// catalogue identity always travel together, so a stamped name
/// without its species/maturity pair is unrepresentable.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredMob {
    /// The display name: the mob as the game prints it ("<species>
    /// <maturity>", or the bare species when no maturity is set), or the
    /// label the player gave it (see `mob_display`).
    pub name: String,
    pub species: String,
    pub maturity: String,
}

impl DeclaredMob {
    /// Build the declaration from a species/maturity pair, deriving
    /// the display name the way the session-start and reload paths
    /// always have.
    pub fn from_parts(species: String, maturity: String) -> Self {
        let name = crate::mob_display::printed(&species, &maturity);
        DeclaredMob {
            name,
            species,
            maturity,
        }
    }

    /// The declaration for a species/maturity pair under the label the
    /// player gave the mob, when they gave it one.
    pub fn labelled(
        species: String,
        maturity: String,
        names: &crate::mob_display::MobDisplayNames,
    ) -> Self {
        DeclaredMob {
            name: names.shown(&species, &maturity, None),
            species,
            maturity,
        }
    }
}

impl TrackerActor {
    /// Set the typed creature (the settings' declared mob): while the
    /// screen is read it is the backstop, and otherwise every kill takes it.
    /// While a pin stands the typed creature replaces it (pinned by typing):
    /// the player typing a creature means it. Mid-session changes are
    /// allowed by design.
    pub(super) fn set_declared_mob(
        &mut self,
        mob_name: &str,
        species: &str,
        maturity: &str,
    ) -> Result<(), TrackerCommandError> {
        if self.session.active().is_none() {
            return Err(TrackerCommandError::NoActiveSession);
        }
        // The name the caller made is the mob as the game prints it; the
        // player's own label for the mob stands over it.
        let typed = DeclaredMob {
            name: self
                .providers
                .mob_names
                .chosen(species, maturity)
                .unwrap_or_else(|| mob_name.to_string()),
            species: species.to_string(),
            maturity: maturity.to_string(),
        };
        let Some(active) = self.session.active_mut() else {
            return Err(TrackerCommandError::NoActiveSession);
        };
        let replaced = active.mob.pin.is_some();
        if replaced {
            active.mob.pin = Some(Pin {
                mob: typed.clone(),
                by: PinnedBy::Typed,
                provisional_key: None,
            });
        }
        active.mob.typed = Some(typed);
        if replaced {
            self.emit_mob_source_changed();
        }
        Ok(())
    }

    /// Clear the typed creature, returning the released name. A pin stands
    /// (it is released on its own). Idle is a no-op (idle carries no
    /// declaration to release).
    pub(super) fn release_declared_mob(&mut self) -> Option<String> {
        let active = self.session.active_mut()?;
        active
            .mob
            .typed
            .take()
            .map(|declared| declared.name)
            .filter(|name| !name.is_empty())
    }

    /// Pin the creature the screen is reading now (or, when nothing is read,
    /// the one the latest kills were), provided it is still `expected`: the
    /// creature the player saw when they asked. A reading that moved on
    /// since is refused, so a pin never lands on a creature the player did
    /// not choose.
    pub(super) async fn pin_shown(
        &mut self,
        expected: DeclaredMob,
    ) -> Result<DeclaredMob, TrackerCommandError> {
        let reading_screen = self.reading_screen();
        let Some(active) = self.session.active() else {
            return Err(TrackerCommandError::NoActiveSession);
        };
        let Some(detection) = active.detection.as_ref().filter(|_| reading_screen) else {
            return Err(TrackerCommandError::NotReadingScreen);
        };
        let Some((shown, provisional_key)) = detection.standing() else {
            return Err(TrackerCommandError::NothingRead);
        };
        if shown != expected {
            return Err(TrackerCommandError::ReadingChanged);
        }
        self.pin(Pin {
            mob: shown.clone(),
            by: PinnedBy::Read,
            provisional_key,
        })
        .await;
        Ok(shown)
    }

    /// Pin a creature the player typed.
    pub(super) async fn pin_typed(&mut self, mob: DeclaredMob) -> Result<(), TrackerCommandError> {
        if self.session.active().is_none() {
            return Err(TrackerCommandError::NoActiveSession);
        }
        let mob = DeclaredMob::labelled(mob.species, mob.maturity, &self.providers.mob_names);
        self.pin(Pin {
            mob,
            by: PinnedBy::Typed,
            provisional_key: None,
        })
        .await;
        Ok(())
    }

    /// Stand `pin`: the screen stops being read (the kills still waiting
    /// are asked one last time), and every kill from now takes it.
    async fn pin(&mut self, pin: Pin) {
        if let Some(active) = self.session.active_mut() {
            active.mob.pin = Some(pin);
        }
        // Told once: by the stop of the screen's reading, or here.
        if !self.sync_detection().await {
            self.emit_mob_source_changed();
        }
    }

    /// Release the pin: the screen is read again when detection is on.
    /// Answers whether a pin was released.
    pub(super) async fn unpin(&mut self) -> Result<bool, TrackerCommandError> {
        let Some(active) = self.session.active_mut() else {
            return Err(TrackerCommandError::NoActiveSession);
        };
        let released = active.mob.pin.take().is_some();
        let told = self.sync_detection().await;
        if released && !told {
            self.emit_mob_source_changed();
        }
        Ok(released)
    }

    /// Tell the readouts the session's mob source changed.
    pub(super) fn emit_mob_source_changed(&self) {
        let Some(active) = self.session.active() else {
            return;
        };
        let session_id = active.session.id.clone();
        let now = crate::time::naive_to_epoch(self.clock.now());
        self.emit_session_event(
            eo_wire::domain_events::TrackingReason::Updated,
            eo_wire::domain_events::TrackingStatus::Active,
            now,
            Some(&session_id),
        );
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
                MobDetectionGrade::Continuity,
                MobDetectionGrade::ProvisionalContinuity,
                MobDetectionGrade::ProvisionalPinned,
            ]
        );
        assert!(MobDetectionGrade::ProvisionalPinned.provisional());
        let carried: Vec<_> = MobDetectionGrade::ALL
            .into_iter()
            .filter(|grade| grade.carried())
            .collect();
        assert_eq!(
            carried,
            [
                MobDetectionGrade::Continuity,
                MobDetectionGrade::ProvisionalContinuity,
                MobDetectionGrade::ProvisionalPinned,
            ]
        );
        assert!(MobDetectionGrade::ProvisionalLatestRead.provisional());
        assert!(MobDetectionGrade::ProvisionalContinuity.provisional());
        assert!(!MobDetectionGrade::LatestRead.provisional());
        assert!(!MobDetectionGrade::Continuity.provisional());
        let continuing: Vec<_> = MobDetectionGrade::ALL
            .into_iter()
            .filter(|grade| grade.continuity())
            .collect();
        assert_eq!(
            continuing,
            [
                MobDetectionGrade::Continuity,
                MobDetectionGrade::ProvisionalContinuity
            ]
        );
    }

    #[test]
    fn routes_round_trip_and_follow_their_grades_and_pins() {
        for route in MobRoute::ALL {
            assert_eq!(MobRoute::parse(route.as_str()), Some(route));
        }
        assert_eq!(MobRoute::parse("Backstop"), None);
        assert_eq!(MobRoute::parse("latest_read"), None);
        // Every grade detection stamps a kill at names it by the route of
        // the same name; a suggestion names none; an unlisted pin is a
        // pin from a reading.
        for grade in MobDetectionGrade::ALL {
            match MobRoute::of_grade(grade) {
                Some(route) if grade == MobDetectionGrade::ProvisionalPinned => {
                    assert_eq!(route, MobRoute::PinnedRead);
                }
                Some(route) => {
                    assert!(grade.stamps());
                    assert_eq!(route.as_str(), grade.as_str());
                }
                None => assert!(!grade.stamps()),
            }
        }
        assert_eq!(MobRoute::of_pin(PinnedBy::Read), MobRoute::PinnedRead);
        assert_eq!(MobRoute::of_pin(PinnedBy::Typed), MobRoute::PinnedTyped);
    }

    #[test]
    fn a_kill_is_recorded_with_the_pin_else_the_typed_creature_by_whether_the_screen_is_read() {
        let atrox = DeclaredMob::from_parts("Atrox".into(), "Young".into());
        let daikiba = DeclaredMob::from_parts("Daikiba".into(), String::new());
        let unset = MobSource::default();
        for reading in [false, true] {
            assert_eq!(
                unset.record(reading),
                RecordedMob {
                    mob: None,
                    route: None,
                    unlisted_pin: None,
                }
            );
        }
        let typed = MobSource {
            typed: Some(atrox.clone()),
            pin: None,
        };
        // Without screen reading the typed creature is pinned by typing:
        // the declared mob as it always was.
        assert_eq!(
            typed.record(false),
            RecordedMob {
                mob: Some(atrox.clone()),
                route: Some(MobRoute::PinnedTyped),
                unlisted_pin: None,
            }
        );
        assert_eq!(
            typed.record(true),
            RecordedMob {
                mob: Some(atrox.clone()),
                route: Some(MobRoute::Backstop),
                unlisted_pin: None,
            }
        );
        let pinned = MobSource {
            typed: Some(atrox.clone()),
            pin: Some(Pin {
                mob: daikiba.clone(),
                by: PinnedBy::Read,
                provisional_key: None,
            }),
        };
        for reading in [false, true] {
            assert_eq!(
                pinned.record(reading),
                RecordedMob {
                    mob: Some(daikiba.clone()),
                    route: Some(MobRoute::PinnedRead),
                    unlisted_pin: None,
                }
            );
        }
        // A creature on no list pinned from a reading is kept for the
        // review, with the stamp the kill would have had without the pin.
        let gloomrat = DeclaredMob::from_parts("Gloomrat".into(), String::new());
        let unlisted = MobSource {
            typed: Some(atrox.clone()),
            pin: Some(Pin {
                mob: gloomrat.clone(),
                by: PinnedBy::Read,
                provisional_key: Some("GLOOMRAT|4|0".into()),
            }),
        };
        assert_eq!(
            unlisted.record(false),
            RecordedMob {
                mob: Some(gloomrat),
                route: Some(MobRoute::PinnedRead),
                unlisted_pin: Some(("GLOOMRAT|4|0".into(), Some(atrox))),
            }
        );
        // An empty typed name is no creature, as before.
        let empty = MobSource {
            typed: Some(DeclaredMob::from_parts(String::new(), String::new())),
            pin: None,
        };
        assert_eq!(empty.record(false).route, None);
    }

    #[test]
    fn declared_mob_display_name_derivation() {
        let bare = DeclaredMob::from_parts("Atrox".into(), String::new());
        assert_eq!(bare.name, "Atrox");
        let mature = DeclaredMob::from_parts("Atrox".into(), "Old".into());
        assert_eq!(mature.name, "Atrox Old");
    }
}
