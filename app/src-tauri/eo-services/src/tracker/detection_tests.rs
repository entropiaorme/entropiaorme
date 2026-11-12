//! Creature detection through the real actor: looks from a stand-in
//! watcher and loot from the bus, interleaved in time, and the stamps the
//! tracker writes compared with what the script says each kill was.

use std::sync::{Arc, Mutex};

use super::tests::{naive, rig, Rig};
use super::{
    CarriedWeaponProfile, EquipmentLibrary, EquipmentProfile, HarvestGuardrailTools, HuntTracker,
    Providers, TrackingConfig,
};
use crate::bus_events::{
    ActiveToolChangedPayload, BusEvent, CombatPayload, LootGroupPayload, LootItem, LootTag,
};
use crate::game_ui::mob_history::{MobHistory, Sighting};
use crate::game_ui::mob_names::{resolve, CatalogueMob, MobCatalogue, Resolved};
use crate::game_ui::test_support::bundled_profile;
use crate::mob_watch::{MobDetector, MobPulse, PulseSink, WatchEnded};
use crate::time::clock_epoch_ms;
use crate::tracker::{DeclaredMob, MobSourceView, Pin, PinnedBy, TrackerCommandError};

/// The configuration a detection test runs under: the declared mob and
/// the detection setting, switchable mid-session.
struct DetectionConfig {
    declared: Option<(String, String)>,
    detection: Arc<Mutex<bool>>,
}

impl TrackingConfig for DetectionConfig {
    fn session_name(&self) -> String {
        String::new()
    }

    fn session_definition_id(&self) -> Option<i64> {
        None
    }

    fn declared_skill_boost_percent(&self) -> Option<i64> {
        None
    }

    fn manual_mob(&self) -> Option<(String, String)> {
        self.declared.clone()
    }

    fn loot_filter_blacklist(&self) -> Vec<String> {
        Vec::new()
    }

    fn mob_detection_enabled(&self) -> bool {
        *self.detection.lock().unwrap()
    }
}

/// A library pricing every shot at 5 PEC, so kills cycle value and the
/// session is summed.
struct PricedShots;

impl EquipmentLibrary for PricedShots {
    fn weapon_profile(&self, _tool_name: &str) -> EquipmentProfile {
        None
    }

    fn cost_per_shot(&self, _tool_name: &str) -> f64 {
        0.05
    }

    fn carried_weapons(&self) -> Vec<CarriedWeaponProfile> {
        Vec::new()
    }

    fn resolve_harvest_guardrail(&self) -> Option<HarvestGuardrailTools> {
        None
    }
}

/// A watcher that looks at no screen: the test hands the tracker each look
/// through the sink the tracker gave it.
#[derive(Default)]
struct ScriptedWatcher {
    sink: Mutex<Option<(String, PulseSink)>>,
    /// How the tracker asked to be told the watch ended by itself.
    ended: Mutex<Option<WatchEnded>>,
    starts: Mutex<Vec<String>>,
    stops: Mutex<usize>,
    permits: Mutex<usize>,
}

impl MobDetector for ScriptedWatcher {
    fn new_history(&self) -> Option<MobHistory> {
        let profile = bundled_profile();
        Some(MobHistory::new(profile.mob_history, profile.mob_names))
    }

    fn start(&self, session_id: &str, sink: PulseSink, ended: WatchEnded) {
        *self.ended.lock().unwrap() = Some(ended);
        self.starts.lock().unwrap().push(session_id.to_string());
        *self.sink.lock().unwrap() = Some((session_id.to_string(), sink));
    }

    fn stop(&self) {
        *self.stops.lock().unwrap() += 1;
        *self.sink.lock().unwrap() = None;
    }

    fn watches(&self, session_id: &str) -> bool {
        self.sink
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|(watched, _)| watched == session_id)
    }

    fn permit_capture(&self) {
        *self.permits.lock().unwrap() += 1;
    }
}

struct Hunt {
    rig: Rig,
    tracker: Arc<HuntTracker>,
    watcher: Arc<ScriptedWatcher>,
    detection: Arc<Mutex<bool>>,
    catalogue: MobCatalogue,
    /// The clock's reading at the start, in epoch milliseconds: every
    /// script time is an offset from it.
    base_ms: i64,
    /// The script's time the clock has been moved to.
    now_ms: i64,
    kills: usize,
    /// When set, each look is followed by a read of what the overlay shows,
    /// counting how often it changed.
    follow_view: Option<(MobSourceView, usize)>,
}

fn hunt(detection: bool) -> Hunt {
    hunt_with(detection, None)
}

/// A hunt whose tracker looks through `detector` instead of the scripted
/// watcher (which then sees nothing).
fn hunt_with(detection: bool, detector: Option<Arc<dyn MobDetector>>) -> Hunt {
    let rig = rig();
    let watcher = Arc::new(ScriptedWatcher::default());
    let detection = Arc::new(Mutex::new(detection));
    let tracker = rig.tracker(Providers {
        config: Arc::new(DetectionConfig {
            declared: Some(("Atrox".into(), "Young".into())),
            detection: detection.clone(),
        }),
        detector: detector.unwrap_or_else(|| watcher.clone()),
        equipment: Arc::new(PricedShots),
        ..Providers::default()
    });
    let names = bundled_profile().mob_names;
    let catalogue = MobCatalogue::new(
        [
            ("Atrox", "Young", 5),
            ("Atrox", "Old", 12),
            ("Daikiba", "Mature", 9),
        ]
        .map(|(name, maturity, level)| CatalogueMob {
            name: name.into(),
            species: Some(name.into()),
            maturity: Some(maturity.into()),
            level: Some(level),
        }),
        Vec::<String>::new(),
        &names,
    );
    let base_ms = clock_epoch_ms(rig.clock.as_ref());
    Hunt {
        rig,
        tracker,
        watcher,
        detection,
        catalogue,
        base_ms,
        now_ms: 0,
        kills: 0,
        follow_view: None,
    }
}

impl Hunt {
    fn start(&self) -> String {
        let id = self.rig.wait(self.tracker.start_session()).unwrap().id;
        self.rig
            .bus
            .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
                tool_name: "Rifle".into(),
                source: None,
            }));
        id
    }

    /// Move the clock to `at` (script milliseconds), never back.
    fn at(&mut self, at: i64) {
        assert!(at >= self.now_ms, "the script runs forwards");
        self.rig
            .clock
            .advance((at - self.now_ms) as f64 / 1000.0)
            .unwrap();
        self.now_ms = at;
    }

    /// One look at `at`: the plate locked then, read as `text` at `level`
    /// with `fill` health, or nothing locked.
    fn look(&mut self, at: i64, plate: Option<(&str, u32, f64)>) {
        self.look_for(None, at, plate);
    }

    fn look_for(&mut self, session: Option<&str>, at: i64, plate: Option<(&str, u32, f64)>) {
        self.at(at);
        let names = bundled_profile().mob_names;
        let sighting = plate.map(|(text, level, fill)| Sighting {
            fill,
            raw_name: Some(text.to_string()),
            resolved: resolve(text, Some(level), &self.catalogue, &names),
        });
        let Some((watched, sink)) = self.watcher.sink.lock().unwrap().clone() else {
            return;
        };
        assert!(
            sink(MobPulse {
                session_id: session.map_or(watched, str::to_string),
                at_ms: self.base_ms + at,
                sighting,
            }),
            "the tracker took the look"
        );
        if self.follow_view.is_some() {
            let view = self.view();
            if let Some((last, changes)) = self.follow_view.as_mut() {
                if *last != view {
                    *last = view;
                    *changes += 1;
                }
            }
        }
    }

    /// A kill whose line was stamped with the second `logged` falls in and
    /// was read at `arrived` (both script milliseconds); `arrived: None` is a
    /// line not read live.
    fn kill(&mut self, logged: i64, arrived: Option<i64>) {
        if let Some(arrived) = arrived {
            self.at(arrived);
        }
        self.report_kill(logged, arrived);
    }

    /// The same report without moving the clock: a line stamped as read at
    /// `arrived` that reaches the tracker later than that.
    fn report_kill(&mut self, logged: i64, arrived: Option<i64>) {
        self.kills += 1;
        // One priced shot before each kill, so the session cycles value.
        let shot_at = naive("2026-01-01T00:00:00") + chrono::TimeDelta::seconds(logged / 1000);
        self.rig
            .bus
            .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
                amount: 20.0,
                timestamp: shot_at.format("%Y-%m-%dT%H:%M:%S").to_string(),
            }));
        let stamp = naive("2026-01-01T00:00:00") + chrono::TimeDelta::seconds(logged / 1000);
        self.rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
            kind: LootTag,
            source_id: None,
            arrived_ms: arrived.map(|at| self.base_ms + at),
            timestamp: Some(stamp.format("%Y-%m-%dT%H:%M:%S").to_string()),
            items: vec![LootItem {
                item_name: "Animal Hide".into(),
                quantity: 1,
                value_ped: self.kills as f64 / 10.0,
                is_enhancer_shrapnel: false,
            }],
            total_ped: self.kills as f64 / 10.0,
        }));
    }

    /// Let every look sent so far reach the tracker.
    fn settle(&self) {
        self.rig.probe(&self.tracker, |_| ());
    }

    /// Each kill's stamp in order: name, source, grade, text read.
    fn stamps(&self) -> Vec<Stamp> {
        self.settle();
        self.rig
            .wait(self.rig.db.with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT mob_name, mob_stamp_source, mob_detection_grade, mob_read_text \
                     FROM kills ORDER BY timestamp, rowid",
                )?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            }))
            .unwrap()
    }

    /// Each kill's name and route, in order.
    fn routes(&self) -> Vec<(String, Option<String>)> {
        self.settle();
        self.rig
            .wait(self.rig.db.with_reader(|conn| {
                let mut stmt = conn
                    .prepare("SELECT mob_name, mob_route FROM kills ORDER BY timestamp, rowid")?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            }))
            .unwrap()
    }

    /// The kills given the creature being hunted: the creature and the
    /// stamp each was recorded with.
    fn continued(&self) -> Vec<(String, String)> {
        self.settle();
        self.rig
            .wait(self.rig.db.with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT c.mob_name, c.declared_name FROM kill_mob_carried c \
                     JOIN kills k ON k.id = c.kill_id ORDER BY k.timestamp, k.rowid",
                )?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            }))
            .unwrap()
    }

    /// The session's mob source as the readouts show it.
    fn view(&self) -> MobSourceView {
        self.settle();
        self.rig.probe(&self.tracker, |actor| {
            let reading = actor.reading_screen();
            actor
                .session
                .active()
                .map(|active| active.mob_source_view(reading))
                .expect("a session")
        })
    }

    /// A kill nothing was read round: looks with nothing locked from
    /// `from`, the death, and more such looks. Returns when the next look
    /// is due.
    /// The running session's id.
    fn view_session(&self) -> String {
        self.rig.probe(&self.tracker, |actor| {
            actor
                .session
                .active()
                .map(|active| active.session.id.clone())
                .expect("a session")
        })
    }

    fn quiet_kill(&mut self, from: i64) -> i64 {
        let at = self.idle(from, 3);
        let death = at - 500;
        self.kill(death / 1000 * 1000, Some(death + 700));
        self.idle(at + 1_000, 6)
    }

    fn kept(&self) -> Vec<(String, String, String)> {
        self.settle();
        self.rig
            .wait(self.rig.db.with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT d.grade, d.mob_name, d.declared_name FROM kill_mob_detections d \
                     JOIN kills k ON k.id = d.kill_id ORDER BY k.timestamp, k.rowid",
                )?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            }))
            .unwrap()
    }

    /// A fight: the plate read once a second from `from`, `reads` times,
    /// its health falling, then the creature dies half a second after the
    /// last read and its plate is not read again. Its line is stamped with
    /// the second of the death and read 700 ms after it (or, not `live`,
    /// not read live off the log). Returns when the next look is due.
    fn fight(&mut self, from: i64, texts: &[&str], level: u32) -> i64 {
        self.fight_read(from, texts, level, true)
    }

    fn fight_read(&mut self, from: i64, texts: &[&str], level: u32, live: bool) -> i64 {
        let mut at = from;
        for (i, text) in texts.iter().enumerate() {
            let fill = 1.0 - 0.8 * i as f64 / texts.len() as f64;
            self.look(at, Some((text, level, fill)));
            at += 1_000;
        }
        let death = at - 500;
        // The look due as the creature died sees nothing locked.
        self.look(at, None);
        self.kill(death / 1000 * 1000, live.then_some(death + 700));
        at + 1_000
    }

    /// Looks with nothing locked from `from`, `n` of them.
    fn idle(&mut self, from: i64, n: i64) -> i64 {
        for k in 0..n {
            self.look(from + k * 1_000, None);
        }
        from + n * 1_000
    }
}

/// A kill's stamp: name, source, grade, text read.
type Stamp = (String, Option<String>, Option<String>, Option<String>);

fn s(text: &str) -> Option<String> {
    Some(text.to_string())
}

#[test]
fn a_simulated_hunt_stamps_what_the_screen_proved_and_keeps_the_declaration_otherwise() {
    let mut hunt = hunt(true);
    let session = hunt.start();
    assert_eq!(
        *hunt.watcher.starts.lock().unwrap(),
        std::slice::from_ref(&session)
    );

    // What each kill truly was, in order.
    let truth = [
        "Young Atrox",
        "Gloomrat",
        "Mature Daikiba",
        "Old Atrox",
        "Mature Daikiba",
        "Young Atrox",
        "Old Atrox",
    ];
    let mut t = 0;
    // 1. A listed creature read cleanly.
    t = hunt.fight(t, &["Atrox Young"; 5], 5);
    t = hunt.idle(t, 6);
    // 2. A creature on no list, read at its level. Its first reading, at
    //    full health, cannot be told from a fresh lock on another such
    //    creature; the fight is long enough that the player has clearly
    //    gone on with this one by the time it dies.
    t = hunt.fight(
        t,
        &[
            "Gloomrat", "Gloomrat", "Gloomrot", "Gloomrat", "Gloomrat", "Gloomrat",
        ],
        4,
    );
    t = hunt.idle(t, 6);
    // 3. A listed creature, one reading a letter off.
    t = hunt.fight(
        t,
        &[
            "Daikiba Mature",
            "Daikiba Matrue",
            "Daikiba Mature",
            "Daikiba Mature",
        ],
        9,
    );
    t = hunt.idle(t, 6);
    // 4. The watcher stalls round the kill (the game in the background):
    //    nothing is said about it, so the declaration stands.
    for (i, at) in [0, 1_000, 2_000].into_iter().enumerate() {
        hunt.look(t + at, Some(("Atrox Old", 12, 1.0 - 0.3 * i as f64)));
    }
    hunt.kill((t + 2_500) / 1000 * 1000, Some(t + 3_200));
    t = hunt.idle(t + 12_000, 6);
    // 5. Another clean kill.
    t = hunt.fight(t, &["Daikiba Mature"; 3], 9);
    t = hunt.idle(t, 6);
    // 6. Something else killed while the player stays locked on a living
    //    creature (area damage): not that creature, so nothing is said.
    for k in 0..3 {
        hunt.look(
            t + k * 1_000,
            Some(("Atrox Young", 5, 1.0 - 0.1 * k as f64)),
        );
    }
    hunt.kill((t + 2_000) / 1000 * 1000, Some(t + 2_400));
    for k in 3..9 {
        hunt.look(
            t + k * 1_000,
            Some(("Atrox Young", 5, 0.7 - 0.05 * k as f64)),
        );
    }
    t += 9_000;
    t = hunt.idle(t, 6);
    // 7. A kill whose line was not read live (no arrival): never asked.
    t = hunt.fight_read(t, &["Atrox Old"; 3], 12, false);
    hunt.idle(t, 6);

    let stamps = hunt.stamps();
    assert_eq!(stamps.len(), truth.len());
    let declared = (s("Young Atrox"), s("declared"), None, None);
    assert_eq!(
        stamps[0],
        (
            s("Young Atrox").unwrap(),
            s("detected"),
            s("named"),
            s("Atrox Young")
        )
    );
    assert_eq!(
        stamps[1],
        (
            s("Gloomrat").unwrap(),
            s("detected"),
            s("provisional"),
            s("Gloomrat")
        )
    );
    assert_eq!(
        stamps[2],
        (
            s("Mature Daikiba").unwrap(),
            s("detected"),
            s("named"),
            s("Daikiba Mature")
        )
    );
    assert_eq!(stamps[4].0, "Mature Daikiba");
    for i in [3, 5] {
        assert_eq!(
            (
                s(&stamps[i].0),
                stamps[i].1.clone(),
                stamps[i].2.clone(),
                stamps[i].3.clone()
            ),
            declared,
            "kill {} keeps the declaration",
            i + 1
        );
    }

    // The tally against the script's truth.
    let (mut right, mut kept, mut wrong) = (0, 0, 0);
    for (stamp, truth) in stamps.iter().zip(truth) {
        match stamp.1.as_deref() {
            Some("detected") if stamp.0 == truth => right += 1,
            Some("detected") => wrong += 1,
            _ => kept += 1,
        }
    }
    println!(
        "simulated hunt: {} kills, {right} detected right, {kept} kept the declaration, {wrong} WRONG",
        truth.len()
    );
    assert_eq!(wrong, 0);
    assert_eq!(right, 4);
    // Kill 7's line was not read live: its fight was answered for no kill.
    assert_eq!(kept, 3);

    // Every answer is kept with the stamp the declaration gave.
    let kept_rows = hunt.kept();
    assert_eq!(kept_rows.len(), 4);
    assert!(kept_rows
        .iter()
        .all(|(_, _, declared)| declared == "Young Atrox"));

    // Stopping the session stops the watcher; its looks no longer reach it.
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert_eq!(*hunt.watcher.stops.lock().unwrap(), 1);
}

#[test]
fn a_kill_unanswered_when_the_session_stops_keeps_its_declaration() {
    let mut hunt = hunt(true);
    hunt.start();
    let t = hunt.fight(0, &["Atrox Young"; 4], 5);
    // Stopped before the screen has been watched long enough past the
    // report: no answer, the declaration stands.
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert!(t > 0);
    assert_eq!(
        hunt.stamps(),
        [(s("Young Atrox").unwrap(), s("declared"), None, None)]
    );
    assert!(hunt.kept().is_empty());
}

#[test]
fn the_lowest_grade_is_a_suggestion_the_kill_does_not_take() {
    let mut hunt = hunt(true);
    hunt.start();
    // A Daikiba fought and left, then an Old Atrox locked for two looks
    // that stops a pulse before the screen goes empty: both were read in the
    // kill's moments, the Atrox was not read on the last look before the
    // report, and the player had not been on it long enough to have left
    // the Daikiba clearly. The screen leans nowhere: the creature read last
    // is only suggested.
    hunt.look(0, Some(("Daikiba Mature", 9, 0.9)));
    hunt.look(1_000, Some(("Daikiba Mature", 9, 0.6)));
    hunt.look(2_000, Some(("Daikiba Mature", 9, 0.3)));
    hunt.look(3_000, None);
    hunt.look(4_000, Some(("Atrox Old", 12, 0.9)));
    hunt.look(5_000, Some(("Atrox Old", 12, 0.5)));
    hunt.look(6_000, None);
    hunt.kill(6_000, Some(6_500));
    hunt.idle(7_000, 4);
    assert_eq!(
        hunt.stamps(),
        [(s("Young Atrox").unwrap(), s("declared"), None, None)],
        "the kill keeps its declared mob"
    );
    assert_eq!(
        hunt.kept(),
        [(
            "latest_read".to_string(),
            "Old Atrox".to_string(),
            "Young Atrox".to_string()
        )],
        "exactly one suggestion, at the lowest grade"
    );
}

#[test]
fn a_failure_inside_the_history_ends_detection_and_the_session_carries_on() {
    let mut hunt = hunt(true);
    hunt.start();
    hunt.look(0, None);
    hunt.rig.probe(&hunt.tracker, |actor| {
        let active = actor.session.active_mut().expect("a session");
        active.detection.as_mut().expect("watching").fail_next_look = true;
    });
    hunt.look(1_000, None);
    hunt.settle();
    assert_eq!(
        *hunt.watcher.stops.lock().unwrap(),
        1,
        "the watcher stopped"
    );
    let watching = hunt.rig.probe(&hunt.tracker, |actor| {
        actor.session.active().map(|a| a.detection.is_some())
    });
    assert_eq!(
        watching,
        Some(false),
        "detection dropped, the session goes on"
    );
    // A kill still records, with its declaration.
    hunt.kill(2_000, Some(2_400));
    assert_eq!(
        hunt.stamps(),
        [(s("Young Atrox").unwrap(), s("declared"), None, None)]
    );
    assert!(hunt.tracker.is_tracking());
}

#[test]
fn the_stop_sums_the_session_from_its_detected_stamps() {
    let mut hunt = hunt(true);
    let session = hunt.start();
    hunt.follow_view = Some((hunt.view(), 0));
    let updates = Arc::new(Mutex::new(0usize));
    let counted = updates.clone();
    hunt.rig.bus.add_tap(move |event| {
        if let BusEvent::TrackingSessionUpdated(update) = event {
            if update.payload.reason == eo_wire::domain_events::TrackingReason::Updated {
                *counted.lock().unwrap() += 1;
            }
        }
    });
    // Three kills of a creature on no list, and one of the declared mob
    // the screen did not see.
    let mut t = 0;
    for _ in 0..3 {
        t = hunt.fight(
            t,
            &["Gloomrat", "Gloomrat", "Gloomrat", "Gloomrat", "Gloomrat"],
            4,
        );
        t = hunt.idle(t, 6);
    }
    hunt.kill(t / 1000 * 1000, None);
    hunt.idle(t + 1_000, 2);
    hunt.settle();
    // Each change to what the overlay shows of the screen tells the
    // readout to re-read, and so does each re-stamp, once.
    let view_changes = hunt.follow_view.as_ref().map_or(0, |(_, changes)| *changes);
    assert!(view_changes > 0, "the reading came and went");
    assert_eq!(
        *updates.lock().unwrap(),
        3 + view_changes,
        "each re-stamp tells the readout to re-read"
    );
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert_eq!(
        hunt.rig.scalar_string(
            "SELECT dominant_mob FROM session_summaries WHERE session_id = ?",
            &[&session],
        ),
        "Gloomrat",
        "an unlisted creature is a creature to the summary"
    );
    assert_eq!(
        hunt.rig.scalar_i64(
            "SELECT COALESCE(SUM(kills), 0) FROM session_kill_rollups \
             WHERE session_id = ? AND mob_species = 'Gloomrat'",
            &[&session],
        ),
        3
    );
}

#[test]
fn with_detection_off_nothing_watches_and_every_kill_keeps_its_declaration() {
    let mut hunt = hunt(false);
    hunt.start();
    assert!(
        hunt.watcher.starts.lock().unwrap().is_empty(),
        "never started"
    );
    // A kill read live, with no history to ask.
    hunt.kill(1_000, Some(1_600));
    assert_eq!(
        hunt.stamps(),
        [(s("Young Atrox").unwrap(), s("declared"), None, None)]
    );
    assert!(hunt.kept().is_empty());

    // Turned on mid-session: the watcher starts, and the player's turning
    // it on lets it ask for a capture again.
    *hunt.detection.lock().unwrap() = true;
    hunt.rig.wait(hunt.tracker.reload_config());
    assert_eq!(hunt.watcher.starts.lock().unwrap().len(), 1);
    assert_eq!(*hunt.watcher.permits.lock().unwrap(), 1);
    // A settings change that leaves it on changes nothing.
    hunt.rig.wait(hunt.tracker.reload_config());
    assert_eq!(hunt.watcher.starts.lock().unwrap().len(), 1);
    assert_eq!(*hunt.watcher.permits.lock().unwrap(), 1);

    // Turned off again: the watcher stops at once.
    *hunt.detection.lock().unwrap() = false;
    hunt.rig.wait(hunt.tracker.reload_config());
    assert_eq!(*hunt.watcher.stops.lock().unwrap(), 1);
    assert!(hunt.watcher.sink.lock().unwrap().is_none());
}

#[test]
fn a_look_taken_for_another_session_is_dropped() {
    let mut hunt = hunt(true);
    hunt.start();
    for k in 0..4 {
        hunt.look_for(
            Some("an-ended-session"),
            k * 1_000,
            Some(("Atrox Old", 12, 0.9)),
        );
    }
    hunt.look_for(Some("an-ended-session"), 4_000, None);
    hunt.kill(3_000, Some(4_200));
    for k in 5..12 {
        hunt.look(k * 1_000, None);
    }
    assert_eq!(
        hunt.stamps(),
        [(s("Young Atrox").unwrap(), s("declared"), None, None)],
        "the other session's looks name nothing here"
    );
}

#[test]
fn the_names_read_resolve_as_the_script_means() {
    // The script's misreadings stay within what the reader takes as the
    // creature, and the unlisted name is new: the hunt above tests the
    // tracker, not the matcher.
    let hunt = hunt(true);
    let names = bundled_profile().mob_names;
    assert!(matches!(
        resolve("Daikiba Matrue", Some(9), &hunt.catalogue, &names),
        Resolved::Known { .. }
    ));
    assert!(matches!(
        resolve("Gloomrot", Some(4), &hunt.catalogue, &names),
        Resolved::New { .. }
    ));
}

fn mob(species: &str, maturity: &str) -> DeclaredMob {
    DeclaredMob::from_parts(species.into(), maturity.into())
}

fn route(name: &str, route: Option<&str>) -> (String, Option<String>) {
    (name.to_string(), route.map(str::to_string))
}

#[test]
fn each_kill_is_named_by_the_screen_then_continuity_then_the_backstop_then_nothing() {
    let mut hunt = hunt(true);
    hunt.start();
    let mut t = 0;
    // 1 to 3: the screen reads each kill's own creature clearly.
    for _ in 0..3 {
        t = hunt.fight(t, &["Daikiba Mature"; 4], 9);
        t = hunt.idle(t, 6);
    }
    // 4: nothing is read round the kill, and the backstop is set: the
    //    creature being hunted still wins over it.
    t = hunt.quiet_kill(t);
    // 5: long after, continuity no longer applies: the backstop.
    t = hunt.idle(t, 125);
    t = hunt.quiet_kill(t);
    // 6: with no backstop either, nothing names the kill.
    assert_eq!(
        hunt.rig.wait(hunt.tracker.release_declared_mob()),
        Some("Young Atrox".to_string())
    );
    hunt.quiet_kill(t);

    assert_eq!(
        hunt.routes(),
        [
            route("Mature Daikiba", Some("named")),
            route("Mature Daikiba", Some("named")),
            route("Mature Daikiba", Some("named")),
            route("Mature Daikiba", Some("continuity")),
            route("Young Atrox", Some("backstop")),
            route("Unknown", None),
        ]
    );
    let stamps = hunt.stamps();
    // The continuity kill is detected, but carries no grade or text read:
    // its evidence was an earlier kill's.
    assert_eq!(
        stamps[3],
        (s("Mature Daikiba").unwrap(), s("detected"), None, None)
    );
    assert_eq!(
        stamps[4],
        (s("Young Atrox").unwrap(), s("declared"), None, None)
    );
    assert_eq!(stamps[5], (s("Unknown").unwrap(), None, None, None));
    // The screen's answers are kept with what was read; the continuity
    // kill apart, with the stamp it was recorded with.
    assert_eq!(hunt.kept().len(), 3);
    assert_eq!(
        hunt.continued(),
        [("Mature Daikiba".to_string(), "Young Atrox".to_string())]
    );
}

#[test]
fn a_pin_from_the_screen_or_by_typing_names_every_kill_until_it_is_released() {
    let mut hunt = hunt(true);
    hunt.start();
    for k in 0..3 {
        hunt.look(k * 1_000, Some(("Atrox Old", 12, 1.0 - 0.1 * k as f64)));
    }
    let view = hunt.view();
    assert!(view.reading_screen);
    assert_eq!(view.reading, Some(mob("Atrox", "Old")));
    assert_eq!(view.typed, Some(mob("Atrox", "Young")));
    assert_eq!(view.pin, None);

    // A pin of a creature other than the one read is refused, and changes
    // nothing: the screen read on.
    assert!(matches!(
        hunt.rig
            .wait(hunt.tracker.pin_shown(mob("Daikiba", "Mature"))),
        Err(TrackerCommandError::ReadingChanged)
    ));
    assert_eq!(*hunt.watcher.stops.lock().unwrap(), 0);

    // Pin what is read: the watch stops at once.
    assert_eq!(
        hunt.rig.wait(hunt.tracker.pin_shown(mob("Atrox", "Old"))),
        Ok(mob("Atrox", "Old"))
    );
    assert_eq!(*hunt.watcher.stops.lock().unwrap(), 1);
    assert!(hunt.watcher.sink.lock().unwrap().is_none());
    let view = hunt.view();
    assert!(!view.reading_screen);
    assert_eq!(view.reading, None);
    assert_eq!(
        view.pin,
        Some(Pin {
            mob: mob("Atrox", "Old"),
            by: PinnedBy::Read,
            provisional_key: None,
        })
    );
    // A kill takes the pin at once, and is no detection.
    hunt.kill(5_000, Some(5_600));
    // While pinned there is no reading to pin from.
    assert!(matches!(
        hunt.rig.wait(hunt.tracker.pin_shown(mob("Atrox", "Old"))),
        Err(TrackerCommandError::NotReadingScreen)
    ));
    // A settings change leaves the pin, and the watch, as they are.
    hunt.rig.wait(hunt.tracker.reload_config());
    assert_eq!(hunt.watcher.starts.lock().unwrap().len(), 1);

    // Pin by typing replaces it.
    hunt.rig
        .wait(hunt.tracker.pin_typed(mob("Daikiba", "Mature")))
        .unwrap();
    hunt.kill(8_000, Some(8_600));
    assert_eq!(hunt.watcher.starts.lock().unwrap().len(), 1);

    // Release: the screen is read again, from a fresh history.
    assert_eq!(hunt.rig.wait(hunt.tracker.unpin()), Ok(true));
    assert_eq!(hunt.watcher.starts.lock().unwrap().len(), 2);
    assert!(hunt.view().reading_screen);
    hunt.quiet_kill(10_000);
    // A second release has nothing to release.
    assert_eq!(hunt.rig.wait(hunt.tracker.unpin()), Ok(false));
    assert_eq!(hunt.watcher.starts.lock().unwrap().len(), 2);

    assert_eq!(
        hunt.routes(),
        [
            route("Old Atrox", Some("pinned_read")),
            route("Mature Daikiba", Some("pinned_typed")),
            route("Young Atrox", Some("backstop")),
        ]
    );
    let stamps = hunt.stamps();
    for stamp in &stamps[..2] {
        assert_eq!(stamp.1, s("declared"), "a pin is the player's word");
    }
    assert!(hunt.kept().is_empty() && hunt.continued().is_empty());
}

#[test]
fn a_kill_answered_after_the_last_look_still_moves_the_creature_recent_kills_show() {
    let mut hunt = hunt(true);
    hunt.start();
    let mut t = 0;
    for _ in 0..2 {
        t = hunt.fight(t, &["Daikiba Mature"; 4], 9);
        t = hunt.idle(t, 8);
    }
    assert_eq!(hunt.view().continuity, None, "two kills are not yet a run");
    // The third fight's looks all come first, and those after the death
    // too; its line is read off the log only then, with no look after it.
    let mut at = t;
    for i in 0..4 {
        let fill = 1.0 - 0.8 * f64::from(i) / 4.0;
        hunt.look(at, Some(("Daikiba Mature", 9, fill)));
        at += 1_000;
    }
    let death = at - 500;
    hunt.idle(at, 8);
    hunt.settle();
    assert_eq!(hunt.view().continuity, None, "the kill is not logged yet");
    // Read when it was written, reaching the tracker after those looks.
    hunt.report_kill(death / 1000 * 1000, Some(death + 700));
    assert_eq!(hunt.view().continuity, Some(mob("Daikiba", "Mature")));
}

#[test]
fn the_view_holds_a_moments_unclear_reading_and_withholds_a_contradicted_continuity() {
    let mut hunt = hunt(true);
    hunt.start();
    let updates = Arc::new(Mutex::new(0usize));
    let counted = updates.clone();
    hunt.rig.bus.add_tap(move |event| {
        if let BusEvent::TrackingSessionUpdated(update) = event {
            if update.payload.reason == eo_wire::domain_events::TrackingReason::Updated {
                *counted.lock().unwrap() += 1;
            }
        }
    });
    let mut t = 0;
    for _ in 0..3 {
        t = hunt.fight(t, &["Daikiba Mature"; 4], 9);
        t = hunt.idle(t, 8);
    }
    // Nothing read now; the latest kills were all one creature.
    let view = hunt.view();
    assert_eq!(view.reading, None);
    assert_eq!(view.continuity, Some(mob("Daikiba", "Mature")));
    let before = *updates.lock().unwrap();

    // Another creature read: the continuity creature is not shown beside
    // it, and the readout was told once.
    hunt.look(t, Some(("Atrox Old", 12, 1.0)));
    let view = hunt.view();
    assert_eq!(view.reading, Some(mob("Atrox", "Old")));
    assert_eq!(view.continuity, None);
    assert_eq!(*updates.lock().unwrap(), before + 1);
    // A moment with nothing read keeps it, and tells nothing.
    hunt.look(t + 1_000, None);
    hunt.look(t + 2_000, None);
    assert_eq!(hunt.view().reading, Some(mob("Atrox", "Old")));
    assert_eq!(*updates.lock().unwrap(), before + 1);
    // Unread for longer than a reading joins its fight: it is let go.
    hunt.idle(t + 3_000, 6);
    let view = hunt.view();
    assert_eq!(view.reading, None);
    assert_eq!(view.continuity, Some(mob("Daikiba", "Mature")));
    assert_eq!(*updates.lock().unwrap(), before + 2);

    // The same creature read again: both are shown.
    hunt.look(t + 10_000, Some(("Daikiba Mature", 9, 1.0)));
    let view = hunt.view();
    assert_eq!(view.reading, Some(mob("Daikiba", "Mature")));
    assert_eq!(view.continuity, Some(mob("Daikiba", "Mature")));
}

#[test]
fn without_detection_the_typed_creature_is_pinned_by_typing() {
    let mut hunt = hunt(false);
    hunt.start();
    hunt.kill(1_000, Some(1_600));
    assert_eq!(
        hunt.rig.wait(hunt.tracker.release_declared_mob()),
        Some("Young Atrox".to_string())
    );
    hunt.kill(3_000, Some(3_600));
    assert_eq!(
        hunt.routes(),
        [
            route("Young Atrox", Some("pinned_typed")),
            route("Unknown", None)
        ]
    );
    let view = hunt.view();
    assert!(!view.reading_screen);
    assert_eq!((view.reading, view.continuity), (None, None));
}

impl Hunt {
    /// Count the readouts' invalidations from now on.
    fn count_updates(&self) -> Arc<Mutex<usize>> {
        let updates = Arc::new(Mutex::new(0usize));
        let counted = updates.clone();
        self.rig.bus.add_tap(move |event| {
            if let BusEvent::TrackingSessionUpdated(update) = event {
                if update.payload.reason == eo_wire::domain_events::TrackingReason::Updated {
                    *counted.lock().unwrap() += 1;
                }
            }
        });
        updates
    }
}

#[test]
fn a_watch_that_ends_by_itself_leaves_the_typed_creature_pinned_and_says_so_once() {
    let mut hunt = hunt(true);
    let session = hunt.start();
    assert!(hunt.view().reading_screen);
    let updates = hunt.count_updates();
    // The desktop stopped capture: the watch ends without the tracker
    // asking, and tells it so.
    let ended = hunt.watcher.ended.lock().unwrap().clone().expect("asked");
    *hunt.watcher.sink.lock().unwrap() = None;
    ended(&session);
    let view = hunt.view();
    assert!(!view.reading_screen);
    assert_eq!((view.reading, view.continuity), (None, None));
    assert_eq!(*updates.lock().unwrap(), 1, "the readouts are told once");
    let detecting = hunt.rig.probe(&hunt.tracker, |actor| {
        actor
            .session
            .active()
            .map(|active| active.detection.is_some())
    });
    assert_eq!(detecting, Some(false), "detection ends with its watch");
    // Told again (a late or repeated message): nothing more.
    ended(&session);
    hunt.settle();
    assert_eq!(*updates.lock().unwrap(), 1);
    assert!(matches!(
        hunt.rig.wait(hunt.tracker.pin_shown(mob("Atrox", "Young"))),
        Err(TrackerCommandError::NotReadingScreen)
    ));
    hunt.kill(2_000, Some(2_600));
    assert_eq!(
        hunt.routes(),
        [route("Young Atrox", Some("pinned_typed"))],
        "no detecting state: the typed creature is the player's word"
    );
}

#[test]
fn turning_detection_off_or_on_mid_session_tells_the_readouts_once_each() {
    let hunt = hunt(true);
    hunt.start();
    let updates = hunt.count_updates();
    *hunt.detection.lock().unwrap() = false;
    hunt.rig.wait(hunt.tracker.reload_config());
    assert_eq!(*updates.lock().unwrap(), 1);
    assert!(!hunt.view().reading_screen);
    // A settings change that leaves it off tells nothing.
    hunt.rig.wait(hunt.tracker.reload_config());
    assert_eq!(*updates.lock().unwrap(), 1);
    *hunt.detection.lock().unwrap() = true;
    hunt.rig.wait(hunt.tracker.reload_config());
    assert_eq!(*updates.lock().unwrap(), 2);
    assert!(hunt.view().reading_screen);
}

#[test]
fn a_pin_stands_whether_detection_is_on_or_off_until_it_is_released_or_replaced() {
    let mut hunt = hunt(true);
    hunt.start();
    hunt.rig
        .wait(hunt.tracker.pin_typed(mob("Daikiba", "Mature")))
        .unwrap();
    // Detection turned off: the pin stands.
    *hunt.detection.lock().unwrap() = false;
    hunt.rig.wait(hunt.tracker.reload_config());
    hunt.kill(1_000, Some(1_600));
    let view = hunt.view();
    assert!(!view.reading_screen);
    assert_eq!(view.pin.map(|pin| pin.mob), Some(mob("Daikiba", "Mature")));
    // Released: the typed creature names kills, as the declared mob did.
    assert_eq!(hunt.rig.wait(hunt.tracker.unpin()), Ok(true));
    hunt.kill(3_000, Some(3_600));
    // Pinned again, then a creature typed: it replaces the pin, never sits
    // as a backstop behind it.
    hunt.rig
        .wait(hunt.tracker.pin_typed(mob("Daikiba", "Mature")))
        .unwrap();
    hunt.rig
        .wait(hunt.tracker.set_declared_mob("Old Atrox", "Atrox", "Old"))
        .unwrap();
    let view = hunt.view();
    assert_eq!(
        view.pin,
        Some(Pin {
            mob: mob("Atrox", "Old"),
            by: PinnedBy::Typed,
            provisional_key: None,
        })
    );
    // Detection on again: the pin still stands, and nothing is watched.
    *hunt.detection.lock().unwrap() = true;
    hunt.rig.wait(hunt.tracker.reload_config());
    assert_eq!(hunt.watcher.starts.lock().unwrap().len(), 1);
    hunt.kill(5_000, Some(5_600));
    assert_eq!(
        hunt.routes(),
        [
            route("Mature Daikiba", Some("pinned_typed")),
            route("Young Atrox", Some("pinned_typed")),
            route("Old Atrox", Some("pinned_typed")),
        ]
    );
}

#[test]
fn a_creature_on_no_list_pinned_from_the_screen_is_kept_for_the_review() {
    let mut hunt = hunt(true);
    hunt.start();
    for k in 0..3 {
        hunt.look(k * 1_000, Some(("Gloomrat", 4, 1.0 - 0.1 * k as f64)));
    }
    let shown = mob("Gloomrat", "");
    assert_eq!(hunt.view().reading, Some(shown.clone()));
    hunt.rig.wait(hunt.tracker.pin_shown(shown)).unwrap();
    let key = hunt
        .view()
        .pin
        .and_then(|pin| pin.provisional_key)
        .expect("the session's key for it");
    hunt.kill(5_000, Some(5_600));
    assert_eq!(hunt.routes(), [route("Gloomrat", Some("pinned_read"))]);
    let kept: Vec<(String, String, Option<String>, String)> = hunt
        .rig
        .wait(hunt.rig.db.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT grade, mob_name, provisional_key, declared_name FROM kill_mob_carried",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }))
        .unwrap();
    assert_eq!(
        kept,
        [(
            "provisional_pinned".to_string(),
            "Gloomrat".to_string(),
            Some(key),
            "Young Atrox".to_string()
        )]
    );
    // The kill itself stays the player's word.
    assert_eq!(hunt.stamps()[0].1, s("declared"));
}

/// A wait the test grants looks through, driven entirely by the test: each
/// credit (granted before the look) lets one look be taken; with none left
/// it waits only for the watch to be stopped, and then ends.
fn granted_wait(credits: Arc<Mutex<usize>>) -> crate::mob_watch::PulseWait {
    Arc::new(move |_due, stopped: &std::sync::mpsc::Receiver<()>| {
        {
            let mut left = credits.lock().unwrap();
            if *left > 0 {
                *left -= 1;
                return true;
            }
        }
        // Nothing is ever sent: this returns when the watch's sender is
        // dropped, that is when it is stopped.
        stopped.recv().is_ok()
    })
}

#[test]
fn nothing_is_captured_while_a_creature_is_pinned_and_capture_resumes_on_release() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use crate::clock::MockClock;
    use crate::game_ui::seams::GameFrame;
    use crate::game_ui::test_support::blank;
    use crate::mob_watch::{CatalogueCell, MobWatch, MobWatchProviders};

    // Every wait for a worker below is bounded only so that a fault fails
    // the test instead of hanging it; what is asserted never depends on
    // timing.
    const BOUND: Duration = Duration::from_secs(60);
    let captures = Arc::new(AtomicUsize::new(0));
    let (shot, shots) = std::sync::mpsc::channel::<()>();
    let shot = Mutex::new(shot);
    // Two looks granted for the first watch, before it starts.
    let credits = Arc::new(Mutex::new(2usize));
    let counted = captures.clone();
    let watch = MobWatch::new(
        MobWatchProviders {
            frame: Arc::new(move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = shot.lock().unwrap().send(());
                Ok(GameFrame::whole(blank(64, 48, [20, 20, 20]), (0, 0)))
            }),
            enabled: Arc::new(|| true),
            profile: Some(Arc::new(bundled_profile())),
            read_text: Some(Arc::new(|_| None)),
            catalogue: Arc::new(CatalogueCell::default()),
            wait: Some(granted_wait(credits.clone())),
            ..MobWatchProviders::default()
        },
        Arc::new(MockClock::new(None, 100.0)),
    );
    let detector: Arc<dyn MobDetector> = Arc::new(watch.clone());
    let mut hunt = hunt_with(true, Some(detector));
    hunt.start();
    // The worker takes exactly its two looks, then waits to be stopped.
    for _ in 0..2 {
        shots.recv().expect("the watch captures while detecting");
    }
    assert_eq!(watch.watching(), Some(hunt.view_session()));

    // Pinned: the watch ends, and its worker with it.
    hunt.rig
        .wait(hunt.tracker.pin_typed(mob("Atrox", "Old")))
        .unwrap();
    assert!(watch.wait_for_workers(BOUND));
    assert_eq!(watch.watching(), None);
    assert_eq!(captures.load(Ordering::SeqCst), 2);
    // Looks granted while pinned are never taken: no worker is there to
    // take them. A kill, and the setting turned off and on, start none.
    *credits.lock().unwrap() = 3;
    hunt.kill(5_000, Some(5_600));
    *hunt.detection.lock().unwrap() = false;
    hunt.rig.wait(hunt.tracker.reload_config());
    *hunt.detection.lock().unwrap() = true;
    hunt.rig.wait(hunt.tracker.reload_config());
    hunt.settle();
    assert_eq!(watch.watching(), None);
    assert!(watch.wait_for_workers(Duration::ZERO), "no worker runs");
    assert_eq!(*credits.lock().unwrap(), 3, "no look was taken");
    assert_eq!(
        captures.load(Ordering::SeqCst),
        2,
        "zero captures between pin and release"
    );

    // Released: the watch resumes and takes the looks granted to it.
    assert_eq!(hunt.rig.wait(hunt.tracker.unpin()), Ok(true));
    for _ in 0..3 {
        shots.recv().expect("the watch resumes after release");
    }
    assert_eq!(captures.load(Ordering::SeqCst), 5);
    assert_eq!(hunt.routes(), [route("Old Atrox", Some("pinned_typed"))]);

    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert!(watch.wait_for_workers(BOUND));
    assert_eq!(captures.load(Ordering::SeqCst), 5);
}
