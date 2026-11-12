//! Creature detection's half in the tracker: the running session's history
//! of the locked plate, fed by the creature watcher's looks, and the one
//! place a kill's stamp is changed to the creature detection named.
//!
//! A kill is recorded the moment its loot settles, with the declared mob
//! exactly as before detection existed. Whether the screen named the
//! creature can only be told once it has been watched for a while after
//! the kill's report arrived, so the kill waits in the session's list of
//! kills to answer, and is asked again on each later look until the
//! history answers or gives up. Then:
//!
//! - a creature at one of the upper four grades re-stamps the kill as
//!   detected, with its grade and the text read, and the stamp it had
//!   from the declaration is kept beside the answer, for the review to
//!   put back;
//! - the lowest grade leaves the kill as it is and keeps the answer as a
//!   suggestion for the review;
//! - no creature leaves the kill as it is, and keeps nothing.
//!
//! Only a kill read live off the log, carrying when its line arrived, is
//! asked about: a log replayed or recorded has no screen beside it. With
//! detection off, or no watcher to look, the session has no history and
//! every kill keeps its declared stamp.

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::game_ui::mob_history::{Creature, KillReport, KillStamp, MobHistory, MobStamp};
use crate::mob_watch::{MobPulse, PulseSink};
use crate::time::clock_epoch_ms;

use super::actor::{TrackerActor, TrackerMsg};
use super::mob::{DeclaredMob, MobDetectionGrade, MobStampSource};

/// The running session's detection state.
pub(super) struct SessionDetection {
    history: MobHistory,
    /// Kills recorded but not yet answered, oldest first.
    pending: Vec<PendingKill>,
    /// Makes the next look fail inside the history, to prove a failure
    /// there is contained.
    #[cfg(test)]
    pub(super) fail_next_look: bool,
}

impl SessionDetection {
    pub(super) fn new(history: MobHistory) -> Self {
        Self {
            history,
            pending: Vec::new(),
            #[cfg(test)]
            fail_next_look: false,
        }
    }
}

/// A kill waiting for the history's answer.
#[derive(Debug, Clone)]
struct PendingKill {
    kill_id: String,
    report: KillReport,
    declared: StampedMob,
}

/// A kill's mob stamp: the three mob columns and where they came from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StampedMob {
    pub(crate) name: String,
    pub(crate) species: String,
    pub(crate) maturity: String,
    pub(crate) source: Option<MobStampSource>,
}

/// One answer detection gave a kill, as it is kept.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KillDetection {
    pub(crate) kill_id: String,
    pub(crate) session_id: String,
    pub(crate) grade: MobDetectionGrade,
    /// The creature named, as a kill's stamp would carry it.
    pub(crate) creature: StampedMob,
    /// The session's key for a creature on no list.
    pub(crate) provisional_key: Option<String>,
    pub(crate) read_text: String,
    /// When the creature's plate was last read before the kill (epoch
    /// seconds).
    pub(crate) read_at: f64,
    /// The stamp the declaration gave the kill.
    pub(crate) declared: StampedMob,
}

impl KillDetection {
    /// The answer for `pending`, from the history's stamp. The grade is
    /// the stamp's own, never raised. None for the creature being hunted,
    /// which is not recorded: such a kill keeps its declared stamp, as an
    /// unnamed one does.
    fn of(pending: &PendingKill, session_id: &str, stamp: MobStamp) -> Option<Self> {
        let (grade, creature, provisional_key) = match &stamp {
            MobStamp::Named { creature, .. } => (MobDetectionGrade::Named, listed(creature), None),
            MobStamp::Guess { creature, .. } => (MobDetectionGrade::Guess, listed(creature), None),
            MobStamp::LatestRead { creature, .. } => {
                (MobDetectionGrade::LatestRead, listed(creature), None)
            }
            MobStamp::Provisional { key, spelling, .. } => (
                MobDetectionGrade::Provisional,
                unlisted(spelling),
                Some(key.clone()),
            ),
            MobStamp::ProvisionalGuess { key, spelling, .. } => (
                MobDetectionGrade::ProvisionalGuess,
                unlisted(spelling),
                Some(key.clone()),
            ),
            MobStamp::ProvisionalLatestRead { key, spelling, .. } => (
                MobDetectionGrade::ProvisionalLatestRead,
                unlisted(spelling),
                Some(key.clone()),
            ),
            MobStamp::Continuity { .. } | MobStamp::ProvisionalContinuity { .. } => return None,
        };
        let evidence = stamp.evidence();
        Some(Self {
            kill_id: pending.kill_id.clone(),
            session_id: session_id.to_string(),
            grade,
            creature,
            provisional_key,
            read_text: evidence.raw_text.trim().to_string(),
            read_at: evidence.last_read_ms as f64 / 1000.0,
            declared: pending.declared.clone(),
        })
    }
}

/// A known creature as a kill's stamp carries it: the species and
/// maturity, and the display name the declaration would give them, so a
/// detected kill groups with a declared kill of the same creature.
fn listed(creature: &Creature) -> StampedMob {
    let (species, maturity) = match creature {
        Creature::Catalogued {
            name,
            species,
            maturity,
        } => (
            species.clone().unwrap_or_else(|| name.clone()),
            maturity.clone().unwrap_or_default(),
        ),
        Creature::Species {
            species, maturity, ..
        } => (species.clone(), maturity.clone().unwrap_or_default()),
        Creature::Confirmed { name } => (name.clone(), String::new()),
    };
    let declared = DeclaredMob::from_parts(species, maturity);
    StampedMob {
        name: declared.name,
        species: declared.species,
        maturity: declared.maturity,
        source: Some(MobStampSource::Detected),
    }
}

/// A creature on no list: its spelling stands for its species too, as a
/// name the player confirms does, so the session's summaries count its
/// kills as a creature's (an empty species reads as no creature at all).
fn unlisted(spelling: &str) -> StampedMob {
    let spelling = spelling.trim().to_string();
    StampedMob {
        name: spelling.clone(),
        species: spelling,
        maturity: String::new(),
        source: Some(MobStampSource::Detected),
    }
}

impl TrackerActor {
    /// Whether the running session should be watching: detection is on
    /// and a session is running.
    fn detection_wanted(&self) -> bool {
        self.session.active().is_some() && self.providers.config.mob_detection_enabled()
    }

    /// Bring the session's detection in line with the setting: start it
    /// when it is on and not running, end it when it is off. Turning the
    /// setting on also lets the watcher ask the desktop again for a
    /// capture it refused before.
    pub(super) async fn sync_detection(&mut self) {
        let enabled = self.providers.config.mob_detection_enabled();
        if enabled && !self.detection_setting {
            self.providers.detector.permit_capture();
        }
        self.detection_setting = enabled;
        let running = self
            .session
            .active()
            .is_some_and(|active| active.detection.is_some());
        if self.detection_wanted() && !running {
            self.start_detection();
        } else if !self.detection_wanted() && running {
            self.end_detection().await;
        }
    }

    /// Start detection for the running session, when it can run here.
    pub(super) fn start_detection(&mut self) {
        if !self.detection_wanted() {
            return;
        }
        let Some(history) = self.providers.detector.new_history() else {
            return;
        };
        let Some(active) = self.session.active_mut() else {
            return;
        };
        active.detection = Some(SessionDetection::new(history));
        let session_id = active.session.id.clone();
        let sender = self.sender.clone();
        // Delivered while the tracker lives; once it is gone the watch ends.
        let sink: PulseSink =
            std::sync::Arc::new(move |pulse| sender.send(TrackerMsg::MobPulse(pulse)).is_ok());
        self.providers.detector.start(&session_id, sink);
    }

    /// End the running session's detection: the kills still waiting are
    /// asked one last time, the watcher stops, and whatever is still
    /// unanswered keeps its declared stamp.
    pub(super) async fn end_detection(&mut self) {
        self.answer_pending().await;
        self.drop_detection();
    }

    /// Stop the watcher and forget the session's history, answering
    /// nothing more: what the stop path does once its own write has
    /// committed, having asked the waiting kills before it.
    pub(super) fn drop_detection(&mut self) {
        let running = self
            .session
            .active()
            .is_some_and(|active| active.detection.is_some());
        if running {
            self.providers.detector.stop();
        }
        if let Some(active) = self.session.active_mut() {
            active.detection = None;
        }
    }

    /// The history failed on what it was handed: detection ends for this
    /// session, which carries on with every kill keeping its declared stamp.
    fn detection_failed(&mut self) {
        tracing::error!(
            target: "eo::tracker",
            "creature detection failed; this session's kills keep the declared mob"
        );
        self.drop_detection();
    }

    /// One look from the watcher: into the history, if it was taken for the
    /// running session; then any kill it settles is answered.
    pub(super) async fn on_mob_pulse(&mut self, pulse: MobPulse) {
        let Some(active) = self.session.active_mut() else {
            return;
        };
        if active.session.id != pulse.session_id {
            return;
        }
        let Some(detection) = active.detection.as_mut() else {
            return;
        };
        #[cfg(test)]
        let fail = std::mem::take(&mut detection.fail_next_look);
        let history = &mut detection.history;
        let observed = catch_unwind(AssertUnwindSafe(|| {
            #[cfg(test)]
            if fail {
                panic!("a history that fails");
            }
            history.observe(pulse.at_ms, pulse.sighting)
        }));
        if observed.is_err() {
            self.detection_failed();
            return;
        }
        self.answer_pending().await;
    }

    /// A kill just recorded with its declared stamp: when it was read live
    /// off the log and the session is watching, it waits for the history's
    /// answer. `logged_ms` is the second its line was stamped with.
    pub(super) async fn await_detection(
        &mut self,
        kill_id: &str,
        logged_ms: i64,
        arrived_ms: Option<i64>,
    ) {
        let Some(arrived_ms) = arrived_ms else {
            return;
        };
        let Some(active) = self.session.active_mut() else {
            return;
        };
        let Some(kill) = active.session.kills.iter().find(|kill| kill.id == kill_id) else {
            return;
        };
        let declared = StampedMob {
            name: kill.mob_name.clone(),
            species: kill.mob_species.clone(),
            maturity: kill.mob_maturity.clone(),
            source: kill.mob_stamp_source,
        };
        let Some(detection) = active.detection.as_mut() else {
            return;
        };
        detection.pending.push(PendingKill {
            kill_id: kill_id.to_string(),
            report: KillReport {
                logged_ms,
                arrived_ms,
            },
            declared,
        });
        self.answer_pending().await;
    }

    /// Ask about every kill still waiting, one last time before the stop
    /// sums the session (see `stop_session`).
    pub(super) async fn answer_waiting_kills(&mut self) {
        self.answer_pending().await;
    }

    /// Ask the history about every kill still waiting, oldest first, and
    /// apply each answer.
    async fn answer_pending(&mut self) {
        let now_ms = clock_epoch_ms(self.clock.as_ref());
        let Some(active) = self.session.active_mut() else {
            return;
        };
        let session_id = active.session.id.clone();
        let Some(SessionDetection {
            history, pending, ..
        }) = active.detection.as_mut()
        else {
            return;
        };
        let mut answers = Vec::new();
        let asked = catch_unwind(AssertUnwindSafe(|| {
            pending.retain(|kill| match history.stamp_for_kill(kill.report, now_ms) {
                KillStamp::NotYet => true,
                KillStamp::Unnamed(_) => false,
                KillStamp::Stamped(stamp) => {
                    answers.extend(KillDetection::of(kill, &session_id, stamp));
                    false
                }
            })
        }));
        if asked.is_err() {
            self.detection_failed();
            return;
        }
        for answer in answers {
            self.apply_detection(answer).await;
        }
    }

    /// The one place a kill takes detection's answer: an upper grade
    /// re-stamps it, in the database (in the same transaction that keeps
    /// the answer) and then in memory, and the live readout is told to
    /// re-read; the lowest grade is kept as a suggestion only. A failed
    /// write changes nothing: the kill keeps its declared stamp everywhere.
    async fn apply_detection(&mut self, answer: KillDetection) {
        let written = answer.clone();
        let result = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                write_detection(&tx, &written)?;
                tx.commit()?;
                Ok(())
            })
            .await;
        if let Err(error) = result {
            tracing::warn!(target: "eo::tracker", %error, "a detected creature could not be saved");
            return;
        }
        if !answer.grade.stamps() {
            return;
        }
        let Some(active) = self.session.active_mut() else {
            return;
        };
        let Some(kill) = active
            .session
            .kills
            .iter_mut()
            .find(|kill| kill.id == answer.kill_id)
        else {
            return;
        };
        kill.mob_name = answer.creature.name.clone();
        kill.mob_species = answer.creature.species.clone();
        kill.mob_maturity = answer.creature.maturity.clone();
        kill.mob_stamp_source = answer.creature.source;
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

/// Keep `answer`, and for an upper grade re-stamp its kill.
pub(crate) fn write_detection(
    tx: &rusqlite::Transaction<'_>,
    answer: &KillDetection,
) -> Result<(), crate::db::DbError> {
    tx.execute(
        "INSERT OR REPLACE INTO kill_mob_detections \
         (kill_id, session_id, grade, mob_name, mob_species, mob_maturity, provisional_key, \
          read_text, read_at, declared_name, declared_species, declared_maturity, \
          declared_source) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            answer.kill_id,
            answer.session_id,
            answer.grade.as_str(),
            answer.creature.name,
            answer.creature.species,
            answer.creature.maturity,
            answer.provisional_key,
            answer.read_text,
            answer.read_at,
            answer.declared.name,
            answer.declared.species,
            answer.declared.maturity,
            answer.declared.source.map(MobStampSource::as_str),
        ],
    )?;
    if answer.grade.stamps() {
        tx.execute(
            "UPDATE kills SET mob_name = ?, mob_species = ?, mob_maturity = ?, \
             mob_stamp_source = 'detected', mob_detection_grade = ?, mob_read_text = ? \
             WHERE id = ? AND session_id = ?",
            rusqlite::params![
                answer.creature.name,
                answer.creature.species,
                answer.creature.maturity,
                answer.grade.as_str(),
                answer.read_text,
                answer.kill_id,
                answer.session_id,
            ],
        )?;
    }
    Ok(())
}
