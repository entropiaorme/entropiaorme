//! Creature detection's half in the tracker: the running session's history
//! of the locked plate, fed by the creature watcher's looks, and the one
//! place a kill's stamp is changed to the creature detection named.
//!
//! The screen is read only while detection is on and the player has not
//! pinned a creature (see `mob`): a pin ends the watch, and releasing it
//! starts a fresh one. A kill is recorded the moment its loot settles,
//! with the typed backstop (or no creature) as its stamp. Whether the
//! screen named the creature can only be told once it has been watched for
//! a while after the kill's report arrived, so the kill waits in the
//! session's list of kills to answer, and is asked again on each later
//! look until the history answers or gives up. Then:
//!
//! - a creature at one of the upper four grades (the screen's own evidence
//!   for the kill) re-stamps the kill as detected, with its grade and the
//!   text read, and the stamp it was recorded with is kept beside the
//!   answer, for the review to put back;
//! - the creature being hunted (continuity) re-stamps the kill as detected
//!   too, by its route alone: its evidence is an earlier kill's, so no text
//!   or time read is kept as this kill's. The stamp it was recorded with is
//!   kept beside it for the review;
//! - the lowest grade leaves the kill as it is and keeps the answer as a
//!   suggestion for the review;
//! - no creature leaves the kill as it is, and keeps nothing.
//!
//! Only a kill read live off the log, carrying when its line arrived, is
//! asked about: a log replayed or recorded has no screen beside it. With
//! detection off, or no watcher to look, the session has no history and
//! every kill keeps the stamp it was recorded with.
//!
//! Each look also moves what the overlay shows of the screen: the creature
//! read now (held through a moment with no plate or no name read, for as
//! long as a reading would still join its fight, but never over a plate
//! whose name read and named nothing) and the one the latest kills were.
//!
//! Every kill answered, named or not, keeps a trace beside it: what the
//! screen's readings came to, whether the creature being hunted was carried
//! onto it, and the readings round it.
//! The readouts are told to re-read only when either changes.

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::game_ui::mob_history::{
    Creature, KillReport, KillStamp, KillTrace, MobHistory, MobIdentity, MobStamp,
};
use crate::game_ui::mob_names::MobCatalogue;
use crate::mob_display::{LabelSource, MobDisplayNames};
use crate::mob_watch::{MobPulse, PulseSink, WatchEnded};
use crate::time::clock_epoch_ms;

use super::actor::{TrackerActor, TrackerMsg};
use super::mob::{DeclaredMob, MobDetectionGrade, MobRoute, MobStampSource};

/// The running session's detection state.
pub(super) struct SessionDetection {
    history: MobHistory,
    /// Kills recorded but not yet answered, oldest first.
    pending: Vec<PendingKill>,
    /// The creature the screen last read strictly, and when.
    reading: Option<(MobIdentity, i64)>,
    /// The creature the latest kills were, as of the latest look.
    continuity: Option<MobIdentity>,
    /// What to show for each mob.
    names: std::sync::Arc<MobDisplayNames>,
    /// Makes the next look fail inside the history, to prove a failure
    /// there is contained.
    #[cfg(test)]
    pub(super) fail_next_look: bool,
}

/// What the screen shows of the hunt, as stamps would carry it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct ScreenView {
    pub(super) reading: Option<DeclaredMob>,
    /// Withheld while the creature read now is another one: the two
    /// would name different creatures as the one being hunted.
    pub(super) continuity: Option<DeclaredMob>,
}

impl SessionDetection {
    pub(super) fn new(history: MobHistory, names: std::sync::Arc<MobDisplayNames>) -> Self {
        Self {
            history,
            names,
            pending: Vec::new(),
            reading: None,
            continuity: None,
            #[cfg(test)]
            fail_next_look: false,
        }
    }

    /// What the screen shows of the hunt now.
    pub(super) fn view(&self) -> ScreenView {
        let reading = self
            .reading
            .as_ref()
            .map(|(who, _)| stamp_of(who, &self.history, &self.names).as_declared());
        let continuity = self
            .continuity
            .as_ref()
            .map(|who| stamp_of(who, &self.history, &self.names).as_declared())
            .filter(|continuity| reading.as_ref().is_none_or(|read| read == continuity));
        ScreenView {
            reading,
            continuity,
        }
    }

    /// The creature a pin from the screen takes: the one read now, else
    /// the one the latest kills were; with the session's key when it is on
    /// no list.
    pub(super) fn standing(&self) -> Option<(DeclaredMob, Option<String>)> {
        let view = self.view();
        let who = if view.reading.is_some() {
            self.reading.as_ref().map(|(who, _)| who)
        } else if view.continuity.is_some() {
            self.continuity.as_ref()
        } else {
            None
        }?;
        let key = match who {
            MobIdentity::Known(_) => None,
            MobIdentity::Unfamiliar { key, .. } => Some(key.clone()),
        };
        Some((stamp_of(who, &self.history, &self.names).as_declared(), key))
    }

    /// Move the shown creatures on with the look taken at `at_ms`, after
    /// the history took it. Answers whether what is shown changed.
    fn follow(&mut self, at_ms: i64) -> bool {
        let before = self.view();
        let hold = self.history.profile().join_gap_ms;
        match self.history.current_creature() {
            Some(who) => self.reading = Some((who, at_ms)),
            // A plate whose name read and named nothing is a creature the
            // reader cannot name: the one read before is not held over it.
            None if self.history.current_unrecognised() => self.reading = None,
            None => {
                if self
                    .reading
                    .as_ref()
                    .is_some_and(|(_, read_ms)| at_ms - read_ms > hold)
                {
                    self.reading = None;
                }
            }
        }
        self.continuity = self.history.continuity(at_ms);
        self.view() != before
    }

    /// Bring the creature the latest kills were up to date at `at_ms`,
    /// after kills were answered: an answer changes it as much as a look
    /// does. Answers whether what is shown changed.
    fn follow_answers(&mut self, at_ms: i64) -> bool {
        let before = self.view();
        self.continuity = self.history.continuity(at_ms);
        self.view() != before
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

impl StampedMob {
    fn as_declared(&self) -> DeclaredMob {
        DeclaredMob {
            name: self.name.clone(),
            species: self.species.clone(),
            maturity: self.maturity.clone(),
        }
    }
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
    /// What the screen read for this kill, for a grade that rests on the
    /// kill's own fight; none for the creature being hunted, which rests on
    /// earlier kills.
    pub(crate) read: Option<KillReading>,
    /// The stamp the kill was recorded with.
    pub(crate) declared: StampedMob,
}

/// The text read off the kill's own fight, and when its plate was last
/// read before the kill (epoch seconds).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KillReading {
    pub(crate) text: String,
    pub(crate) at: f64,
}

impl KillDetection {
    /// The answer kept for a kill pinned to a creature on no list, read
    /// off the screen: `key` is the session's name for it, and `without` the
    /// stamp the kill would have had without the pin. Its spelling is the
    /// screen's, so the review asks for it to be confirmed.
    pub(super) fn pinned_unlisted(
        kill_id: &str,
        session_id: &str,
        creature: &DeclaredMob,
        key: String,
        without: Option<DeclaredMob>,
    ) -> Self {
        let declared = match without {
            Some(mob) => StampedMob {
                name: mob.name,
                species: mob.species,
                maturity: mob.maturity,
                source: Some(MobStampSource::Declared),
            },
            None => StampedMob {
                name: "Unknown".to_string(),
                species: String::new(),
                maturity: String::new(),
                source: None,
            },
        };
        Self {
            kill_id: kill_id.to_string(),
            session_id: session_id.to_string(),
            grade: MobDetectionGrade::ProvisionalPinned,
            creature: StampedMob {
                name: creature.name.clone(),
                species: creature.species.clone(),
                maturity: creature.maturity.clone(),
                source: Some(MobStampSource::Declared),
            },
            provisional_key: Some(key),
            read: None,
            declared,
        }
    }

    /// The answer for `pending`, from the history's stamp. The grade is
    /// the stamp's own, never raised. A continuity stamp's evidence belongs
    /// to an earlier kill, so it is not kept as this kill's reading.
    fn of(
        pending: &PendingKill,
        session_id: &str,
        stamp: MobStamp,
        history: &MobHistory,
        names: &MobDisplayNames,
    ) -> Self {
        let listed = |creature: &Creature| listed(creature, history, names);
        let (grade, creature, provisional_key) = match &stamp {
            MobStamp::Named { creature, .. } => (MobDetectionGrade::Named, listed(creature), None),
            MobStamp::Guess { creature, .. } => (MobDetectionGrade::Guess, listed(creature), None),
            MobStamp::LatestRead { creature, .. } => {
                (MobDetectionGrade::LatestRead, listed(creature), None)
            }
            MobStamp::Continuity { creature, .. } => {
                (MobDetectionGrade::Continuity, listed(creature), None)
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
            MobStamp::ProvisionalContinuity { key, spelling, .. } => (
                MobDetectionGrade::ProvisionalContinuity,
                unlisted(spelling),
                Some(key.clone()),
            ),
        };
        let read = (!grade.carried()).then(|| {
            let evidence = stamp.evidence();
            KillReading {
                text: evidence.raw_text.trim().to_string(),
                at: evidence.last_read_ms as f64 / 1000.0,
            }
        });
        Self {
            kill_id: pending.kill_id.clone(),
            session_id: session_id.to_string(),
            grade,
            creature,
            provisional_key,
            read,
            declared: pending.declared.clone(),
        }
    }
}

/// A creature the history knows, as a kill's stamp carries it.
fn stamp_of(who: &MobIdentity, history: &MobHistory, names: &MobDisplayNames) -> StampedMob {
    match who {
        MobIdentity::Known(creature) => listed(creature, history, names),
        MobIdentity::Unfamiliar { spelling, .. } => unlisted(spelling),
    }
}

/// A known creature as a kill's stamp carries it: recorded by the name its
/// plate prints and its maturity, never by the wider species the bundled
/// list files it under (several creatures share one, and a kill is one
/// creature's); shown under the player's label for it, else as the plate
/// prints it, a number as the plate was read to print it.
fn listed(creature: &Creature, history: &MobHistory, names: &MobDisplayNames) -> StampedMob {
    let (name, maturity) = recorded(creature);
    StampedMob {
        name: names.shown(&name, &maturity, history.printed_maturity(creature)),
        species: name,
        maturity,
        source: Some(MobStampSource::Detected),
    }
}

/// The name and maturity a known creature's kills are recorded under.
fn recorded(creature: &Creature) -> (String, String) {
    match creature {
        Creature::Catalogued { name, maturity, .. } | Creature::Species { name, maturity, .. } => (
            MobCatalogue::printed_name(name).to_string(),
            maturity.clone().unwrap_or_default(),
        ),
        Creature::Confirmed { name } => (name.clone(), String::new()),
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
    /// Whether the running session should be watching: detection is on,
    /// a session is running, and the player has not pinned a creature.
    fn detection_wanted(&self) -> bool {
        self.session
            .active()
            .is_some_and(|active| active.mob.pin.is_none())
            && self.providers.config.mob_detection_enabled()
    }

    /// Whether the screen is being read for the running session: its
    /// detection runs and the watcher is taking looks for it (a watch can
    /// fail to start, capture not running or refused, or end on its own).
    /// Without it there is no detecting state: the typed creature is pinned.
    pub(super) fn reading_screen(&self) -> bool {
        self.session.active().is_some_and(|active| {
            active.detection.is_some() && self.providers.detector.watches(&active.session.id)
        })
    }

    /// Note whether the screen is read now; answers whether that changed
    /// since the readouts were last told.
    pub(super) fn reading_changed(&mut self) -> bool {
        let reading = self.reading_screen();
        match self.session.active_mut() {
            Some(active) if active.reading_told != reading => {
                active.reading_told = reading;
                true
            }
            _ => false,
        }
    }

    /// Bring the session's detection in line with the setting and the
    /// pin: start it when it is wanted and not running, end it when it is
    /// not wanted. Turning the setting on also lets the watcher ask the
    /// desktop again for a capture it refused before. When that changes
    /// whether the screen is read, the readouts are told; answers whether
    /// they were.
    pub(super) async fn sync_detection(&mut self) -> bool {
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
        let changed = self.reading_changed();
        if changed {
            self.emit_mob_source_changed();
        }
        changed
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
        active.detection = Some(SessionDetection::new(
            history,
            self.providers.mob_names.clone(),
        ));
        let session_id = active.session.id.clone();
        let sender = self.sender.clone();
        // Delivered while the tracker lives; once it is gone the watch ends.
        let looks = sender.clone();
        let sink: PulseSink =
            std::sync::Arc::new(move |pulse| looks.send(TrackerMsg::MobPulse(pulse)).is_ok());
        let ended: WatchEnded = std::sync::Arc::new(move |session_id: &str| {
            let _ = sender.send(TrackerMsg::WatchEnded(session_id.to_string()));
        });
        self.providers.detector.start(&session_id, sink, ended);
    }

    /// The watch for `session_id` ended by itself (capture refused or
    /// stopped, detection found off, its looks undeliverable): the screen
    /// is no longer read, so detection ends as it would on the setting
    /// going off, and the readouts are told. A watch already started again
    /// for the session since is left alone.
    pub(super) async fn on_watch_ended(&mut self, session_id: &str) {
        let ours = self
            .session
            .active()
            .is_some_and(|active| active.session.id == session_id && active.detection.is_some());
        if !ours || self.providers.detector.watches(session_id) {
            return;
        }
        self.end_detection().await;
        if self.reading_changed() {
            self.emit_mob_source_changed();
        }
    }

    /// End the running session's detection: the kills still waiting are
    /// asked one last time, the watcher stops, and whatever is still
    /// unanswered keeps the stamp it was recorded with.
    pub(super) async fn end_detection(&mut self) {
        self.answer_pending(clock_epoch_ms(self.clock.as_ref()))
            .await;
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
        if self.reading_changed() {
            self.emit_mob_source_changed();
        }
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
        let at_ms = pulse.at_ms;
        let learned = detection.history.take_printed();
        self.keep_printed(learned).await;
        let Some(detection) = self
            .session
            .active_mut()
            .and_then(|active| active.detection.as_mut())
        else {
            return;
        };
        let looked = detection.follow(at_ms);
        // Asked as of the look itself, not of when the tracker got to it:
        // looks still queued behind this one may be the ones that watched
        // a kill far enough past, so a tracker running behind its watcher
        // must not give that kill up as unwatched.
        self.answer_pending(at_ms).await;
        let answered = self.follow_answers(at_ms);
        if looked || answered {
            self.emit_mob_source_changed();
        }
    }

    /// After kills were answered at `at_ms`: whether the creature the
    /// latest kills were, as shown, changed with them.
    fn follow_answers(&mut self, at_ms: i64) -> bool {
        self.session
            .active_mut()
            .and_then(|active| active.detection.as_mut())
            .is_some_and(|detection| detection.follow_answers(at_ms))
    }

    /// A kill just recorded, while the screen was read, with its typed
    /// backstop (or no creature): when it was read live off the log, it
    /// waits for the history's answer. (A kill recorded while the screen was
    /// not read took the typed creature as the player's word and is never
    /// asked about.) `logged_ms` is the second its line was stamped with.
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
        // Asked as of when its line was read, as a look is as of when it
        // was taken: the looks that watched past it may still be queued
        // behind it when the tracker is running behind.
        self.answer_pending(arrived_ms).await;
        // A kill whose line arrives after the looks that answer it changes
        // what the latest kills were with no further look to show it.
        if self.follow_answers(clock_epoch_ms(self.clock.as_ref())) {
            self.emit_mob_source_changed();
        }
    }

    /// Ask about every kill still waiting, one last time before the stop
    /// sums the session (see `stop_session`).
    pub(super) async fn answer_waiting_kills(&mut self) {
        self.answer_pending(clock_epoch_ms(self.clock.as_ref()))
            .await;
    }

    /// Ask the history about every kill still waiting, oldest first, as of
    /// `now_ms` (past which a kill not yet watched far enough is given up
    /// on), and apply each answer.
    async fn answer_pending(&mut self, now_ms: i64) {
        let Some(active) = self.session.active_mut() else {
            return;
        };
        let session_id = active.session.id.clone();
        let Some(SessionDetection {
            history,
            pending,
            names,
            ..
        }) = active.detection.as_mut()
        else {
            return;
        };
        let mut answers = Vec::new();
        let mut traces = Vec::new();
        let asked = catch_unwind(AssertUnwindSafe(|| {
            pending.retain(|kill| {
                let answer = history.stamp_for_kill(kill.report, now_ms);
                if !matches!(answer, KillStamp::NotYet) {
                    if let Some(trace) = history.take_trace() {
                        traces.push((kill.kill_id.clone(), trace));
                    }
                }
                match answer {
                    KillStamp::NotYet => true,
                    KillStamp::Unnamed(_) => false,
                    KillStamp::Stamped(stamp) => {
                        answers.push(KillDetection::of(kill, &session_id, stamp, history, names));
                        false
                    }
                }
            })
        }));
        if asked.is_err() {
            self.detection_failed();
            return;
        }
        if !traces.is_empty() {
            self.keep_traces(&session_id, traces).await;
        }
        for answer in answers {
            self.apply_detection(answer).await;
        }
    }

    /// Keep how plates were just read to print creatures graded by number
    /// (`04` where the list has `4`): the mob is shown so from now on, and
    /// everywhere it is already recorded, unless the player chose its label.
    /// A printing that cannot be saved is let go: the next reading of it is
    /// kept instead.
    async fn keep_printed(&mut self, learned: Vec<(Creature, String)>) {
        for (creature, printed) in learned {
            let (name, maturity) = recorded(&creature);
            if printed == maturity || self.providers.mob_names.chosen(&name, &maturity).is_some() {
                continue;
            }
            let label = crate::mob_display::printed(&name, &printed);
            let now = crate::time::naive_to_epoch(self.clock.now());
            let (species, of_maturity, text) = (name.clone(), maturity.clone(), label.clone());
            let result = self
                .db
                .with_writer(move |conn| {
                    let tx = conn.transaction()?;
                    crate::mob_display::set_label(
                        &tx,
                        &species,
                        &of_maturity,
                        &text,
                        LabelSource::Plate,
                        now,
                    )?;
                    let labels = crate::mob_display::labels(&tx)?;
                    tx.commit()?;
                    Ok(labels)
                })
                .await;
            match result {
                Ok(labels) => self.providers.mob_names.replace(labels),
                Err(error) => {
                    tracing::warn!(target: "eo::tracker", %error, "a mob's printed name could not be saved");
                    continue;
                }
            }
            if let Some(active) = self.session.active_mut() {
                for kill in &mut active.session.kills {
                    if kill.mob_species == name && kill.mob_maturity == maturity {
                        kill.mob_name = label.clone();
                    }
                }
            }
        }
    }

    /// Keep why each kill answered was answered so. A trace is a diagnostic:
    /// one that cannot be written is let go.
    async fn keep_traces(&mut self, session_id: &str, traces: Vec<(String, KillTrace)>) {
        let session_id = session_id.to_string();
        let result = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                for (kill_id, trace) in &traces {
                    write_trace(&tx, &session_id, kill_id, trace)?;
                }
                tx.commit()?;
                Ok(())
            })
            .await;
        if let Err(error) = result {
            tracing::warn!(target: "eo::tracker", %error, "a detection trace could not be saved");
        }
    }

    /// The one place a kill takes detection's answer: a grade that stamps
    /// re-stamps it, in the database (in the same transaction that keeps
    /// the answer) and then in memory, and the live readout is told to
    /// re-read; the lowest grade is kept as a suggestion only. A failed
    /// write changes nothing: the kill keeps its recorded stamp everywhere.
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
        kill.mob_route = MobRoute::of_grade(answer.grade);
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

/// Keep `trace` beside the kill `kill_id`.
pub(crate) fn write_trace(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    kill_id: &str,
    trace: &KillTrace,
) -> Result<(), crate::db::DbError> {
    let reads: Vec<serde_json::Value> = trace
        .reads
        .iter()
        .map(|r| {
            serde_json::json!({
                "at_ms": r.at_ms,
                "fill": r.fill,
                "text": r.text,
                "named": r.named,
            })
        })
        .collect();
    tx.execute(
        "INSERT OR REPLACE INTO kill_mob_traces (kill_id, session_id, screen, continuity, reads) \
         VALUES (?, ?, ?, ?, ?)",
        rusqlite::params![
            kill_id,
            session_id,
            trace.screen,
            trace.continuity,
            serde_json::Value::Array(reads).to_string()
        ],
    )?;
    Ok(())
}

/// Keep `answer`, and for a grade that stamps re-stamp its kill: an
/// answer from the kill's own fight with its grade and the text read, the
/// creature being hunted by its route alone.
pub(crate) fn write_detection(
    tx: &rusqlite::Transaction<'_>,
    answer: &KillDetection,
) -> Result<(), crate::db::DbError> {
    match &answer.read {
        Some(read) => {
            tx.execute(
                "INSERT OR REPLACE INTO kill_mob_detections \
                 (kill_id, session_id, grade, mob_name, mob_species, mob_maturity, \
                  provisional_key, read_text, read_at, declared_name, declared_species, \
                  declared_maturity, declared_source) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    answer.kill_id,
                    answer.session_id,
                    answer.grade.as_str(),
                    answer.creature.name,
                    answer.creature.species,
                    answer.creature.maturity,
                    answer.provisional_key,
                    read.text,
                    read.at,
                    answer.declared.name,
                    answer.declared.species,
                    answer.declared.maturity,
                    answer.declared.source.map(MobStampSource::as_str),
                ],
            )?;
        }
        None => {
            tx.execute(
                "INSERT OR REPLACE INTO kill_mob_carried \
                 (kill_id, session_id, grade, mob_name, mob_species, mob_maturity, \
                  provisional_key, declared_name, declared_species, declared_maturity, \
                  declared_source) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    answer.kill_id,
                    answer.session_id,
                    answer.grade.as_str(),
                    answer.creature.name,
                    answer.creature.species,
                    answer.creature.maturity,
                    answer.provisional_key,
                    answer.declared.name,
                    answer.declared.species,
                    answer.declared.maturity,
                    answer.declared.source.map(MobStampSource::as_str),
                ],
            )?;
        }
    }
    // A pinned kill was recorded with its stamp; the answer is kept for
    // the review only.
    if answer.grade == MobDetectionGrade::ProvisionalPinned {
        return Ok(());
    }
    let Some(route) = MobRoute::of_grade(answer.grade) else {
        return Ok(());
    };
    // A continuity stamp carries no grade or text read: those columns say
    // what the screen read for the kill itself.
    let (grade, read_text) = match &answer.read {
        Some(read) => (Some(answer.grade.as_str()), Some(read.text.as_str())),
        None => (None, None),
    };
    tx.execute(
        "UPDATE kills SET mob_name = ?, mob_species = ?, mob_maturity = ?, \
         mob_stamp_source = 'detected', mob_detection_grade = ?, mob_read_text = ?, \
         mob_route = ? WHERE id = ? AND session_id = ?",
        rusqlite::params![
            answer.creature.name,
            answer.creature.species,
            answer.creature.maturity,
            grade,
            read_text,
            route.as_str(),
            answer.kill_id,
            answer.session_id,
        ],
    )?;
    Ok(())
}
